package appcore

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"dh/internal/aiprovider"
	"dh/internal/groupmgr"
	"dh/internal/knowledge"
	"dh/internal/moderation"
	"dh/internal/prediction"
	"dh/internal/store"
)

var ErrManagerRequired = errors.New("需要将账号权限设置为管理")

const globalRecallRulesSetting = "rules.global_recall_v1"

// EnsureDefaultRulesTemplate seeds the global ZCG-compatible rules without
// requiring a live gateway. The desktop UI calls this during startup so the
// rules page is useful before 旺商聊 has connected.
func EnsureDefaultRulesTemplate(ctx context.Context, database *store.Store) error {
	if database == nil {
		return errors.New("数据库未打开")
	}
	return (&Manager{Store: database}).ensureDefaultRules(ctx, 0)
}

type Incoming struct {
	Sequence        uint64
	ServerMessageID string
	GroupID         int64
	UserID          int64
	SenderName      string
	Kind            groupmgr.MessageKind
	Text            string
	SentAt          time.Time
}

type ProcessResult struct {
	MessageID     int64
	Stored        bool
	Handled       bool
	MaxSequence   uint64
	RuleMatches   int
	Actions       int
	AIReplied     bool
	CreatedTasks  int
	ProcessingErr string
}

type Status struct {
	Online         bool
	AIReady        bool
	Paused         bool
	AccountID      string
	EnabledGroups  int
	LastSync       time.Time
	LastMessage    time.Time
	LastError      string
	ProcessedCount int64
	ActionCount    int64
}

type Manager struct {
	Store      *store.Store
	Gateway    groupmgr.GroupGateway
	Moderator  *moderation.Engine
	Provider   aiprovider.Provider
	Prediction *prediction.Service
	AccountID  string
	WakeWords  []string

	mu          sync.RWMutex
	moderatorMu sync.Mutex
	status      Status
	groupLocks  map[int64]*sync.Mutex
	aiQueue     chan aiJob
}

type aiJob struct {
	group      groupmgr.Group
	message    groupmgr.Message
	member     *groupmgr.Member
	allowReply bool
}

func New(database *store.Store, gateway groupmgr.GroupGateway, accountID string) *Manager {
	manager := &Manager{
		Store: database, Gateway: gateway, Moderator: moderation.NewEngine(),
		AccountID: accountID, WakeWords: []string{"DH", "小海"}, groupLocks: make(map[int64]*sync.Mutex),
		Prediction: prediction.New(),
		status:     Status{AccountID: accountID}, aiQueue: make(chan aiJob, 256),
	}
	_ = database.MigrateLegacyKnowledge(context.Background(), accountID)
	_ = database.RecoverCardJobs(context.Background(), accountID)
	go manager.aiWorker()
	return manager
}

func (manager *Manager) SetProvider(provider aiprovider.Provider) {
	manager.mu.Lock()
	manager.Provider = provider
	manager.status.AIReady = provider != nil
	manager.mu.Unlock()
}

func (manager *Manager) SetPaused(paused bool) {
	manager.mu.Lock()
	manager.status.Paused = paused
	manager.mu.Unlock()
}

func (manager *Manager) IsPaused() bool {
	manager.mu.RLock()
	defer manager.mu.RUnlock()
	return manager.status.Paused
}

func (manager *Manager) Snapshot() Status {
	manager.mu.RLock()
	defer manager.mu.RUnlock()
	return manager.status
}

func (manager *Manager) SetOnline(online bool, err error) {
	manager.mu.Lock()
	manager.status.Online = online
	if err != nil {
		manager.status.LastError = err.Error()
	} else if online {
		manager.status.LastError = ""
	}
	manager.mu.Unlock()
}

func (manager *Manager) SyncGroups(ctx context.Context) error {
	remote, err := manager.Gateway.ListGroups(ctx)
	if err != nil {
		manager.SetOnline(false, err)
		return err
	}
	local, err := manager.Store.ListGroups(ctx, manager.AccountID, false)
	if err != nil {
		return err
	}
	existing := make(map[int64]groupmgr.Group, len(local))
	for _, group := range local {
		existing[group.GroupID] = group
	}
	now := time.Now().UTC()
	for _, group := range remote {
		if previous, ok := existing[group.GroupID]; ok {
			group.Enabled = previous.Enabled
			group.AIEnabled = previous.AIEnabled
			group.ModerationEnabled = previous.ModerationEnabled
			group.ManualTakeover = previous.ManualTakeover
			group.TaskReminders = previous.TaskReminders
			group.WelcomeMessage = previous.WelcomeMessage
			group.SummarySchedule = previous.SummarySchedule
			group.CreatedAt = previous.CreatedAt
		} else {
			group.Enabled = false
			group.AIEnabled = false
			group.ModerationEnabled = false
			group.TaskReminders = true
			group.WelcomeMessage = "欢迎 @「[成员]」加入群聊，请先查看群规。"
			group.CreatedAt = now
		}
		group.AccountID = manager.AccountID
		group.UpdatedAt, group.SyncedAt = now, now
		if err := manager.Store.UpsertGroup(ctx, group); err != nil {
			return err
		}
	}
	if err := manager.ensureDefaultRules(ctx, 0); err != nil {
		return err
	}
	enabled, _ := manager.Store.ListGroups(ctx, manager.AccountID, true)
	manager.mu.Lock()
	manager.status.Online = true
	manager.status.EnabledGroups = len(enabled)
	manager.status.LastSync = now
	manager.status.LastError = ""
	manager.mu.Unlock()
	return nil
}

func (manager *Manager) SetGroupEnabled(ctx context.Context, groupID int64, enabled bool) error {
	if enabled {
		if err := manager.RequireManager(ctx, groupID); err != nil {
			return err
		}
	}
	groups, err := manager.Store.ListGroups(ctx, manager.AccountID, false)
	if err != nil {
		return err
	}
	for _, group := range groups {
		if group.GroupID != groupID {
			continue
		}
		group.Enabled, group.AIEnabled, group.ModerationEnabled = enabled, enabled, enabled
		group.UpdatedAt = time.Now().UTC()
		if err := manager.Store.UpsertGroup(ctx, group); err != nil {
			return err
		}
		if enabled {
			if err := manager.ensureDefaultRules(ctx, groupID); err != nil {
				return err
			}
		}
		enabledGroups, _ := manager.Store.ListGroups(ctx, manager.AccountID, true)
		manager.mu.Lock()
		manager.status.EnabledGroups = len(enabledGroups)
		manager.mu.Unlock()
		return nil
	}
	return groupmgr.ErrNotFound
}

func (manager *Manager) CanManageGroup(ctx context.Context, groupID int64) (bool, groupmgr.MemberRole, error) {
	selfID, err := strconv.ParseInt(manager.AccountID, 10, 64)
	if err != nil || selfID <= 0 {
		return false, groupmgr.RoleMember, ErrManagerRequired
	}
	member, err := manager.Store.GetMember(ctx, manager.AccountID, groupID, selfID)
	if err != nil {
		return false, groupmgr.RoleMember, ErrManagerRequired
	}
	return isManagerRole(member.Role), member.Role, nil
}

func (manager *Manager) RequireManager(ctx context.Context, groupID int64) error {
	allowed, _, _ := manager.CanManageGroup(ctx, groupID)
	if !allowed {
		if err := manager.SyncMembers(ctx, groupID); err != nil {
			return err
		}
		allowed, _, _ = manager.CanManageGroup(ctx, groupID)
	}
	if !allowed {
		return ErrManagerRequired
	}
	return nil
}

func isManagerRole(role groupmgr.MemberRole) bool {
	return role == groupmgr.RoleOwner || role == groupmgr.RoleAdmin
}

func (manager *Manager) ensureDefaultRules(ctx context.Context, groupID int64) error {
	existing, err := manager.Store.ListRules(ctx, 0)
	if err != nil {
		return err
	}
	if setting, settingErr := manager.Store.GetSetting(ctx, globalRecallRulesSetting); settingErr == nil && setting.Value == "done" && len(existing) > 0 {
		return nil
	}
	byName := make(map[string]groupmgr.ModerationRule, len(existing))
	for _, rule := range existing {
		byName[rule.Name] = rule
	}
	defaults := append(moderation.DefaultZCGRules(0), moderation.DefaultSemanticRules(0)...)
	names := make([]string, 0, len(defaults))
	now := time.Now().UTC()
	for _, rule := range defaults {
		model := ruleToModel(rule)
		model.CreatedAt, model.UpdatedAt = now, now
		if previous, exists := byName[model.Name]; exists {
			model.ID, model.CreatedAt = previous.ID, previous.CreatedAt
		}
		if err := manager.Store.UpsertRule(ctx, &model); err != nil {
			return err
		}
		names = append(names, model.Name)
	}
	if err := manager.Store.DeleteNonGlobalRulesByNames(ctx, names); err != nil {
		return err
	}
	return manager.Store.SetSetting(ctx, groupmgr.AppSetting{Key: globalRecallRulesSetting, Value: "done", UpdatedAt: now})
}

func (manager *Manager) EnsureDefaultRules(ctx context.Context, groupID int64) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	return manager.ensureDefaultRules(ctx, groupID)
}

func ruleToModel(rule moderation.ModerationRule) groupmgr.ModerationRule {
	actions := make([]groupmgr.RuleAction, 0, len(rule.Actions))
	for _, action := range rule.Actions {
		message := action.Message
		if action.Type == moderation.ActionReply {
			message = action.Reply
		}
		actions = append(actions, groupmgr.RuleAction{Type: groupmgr.ActionType(action.Type), Duration: action.Duration, Message: message})
	}
	roles := make([]groupmgr.MemberRole, len(rule.ExemptRoles))
	for index, role := range rule.ExemptRoles {
		roles[index] = groupmgr.MemberRole(role)
	}
	return groupmgr.ModerationRule{
		GroupID: rule.GroupID, Name: rule.Name, Matcher: groupmgr.MatcherType(rule.Matcher), Pattern: rule.Pattern,
		Threshold: rule.Threshold, Count: rule.Count, Window: rule.Window, Cooldown: rule.Cooldown,
		Priority: rule.Priority, Mode: groupmgr.RuleMode(rule.Mode), Enabled: rule.Enabled,
		SemanticThreshold: rule.SemanticThreshold, ExemptRoles: roles,
		ExemptUserIDs: append([]int64(nil), rule.ExemptUserIDs...), Actions: actions,
		CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC(),
	}
}

func (manager *Manager) SyncMembers(ctx context.Context, groupID int64) error {
	return manager.syncMembers(ctx, groupID, false)
}

// HandleMemberJoined is called only after a concrete NIM team-join event.
// Snapshot refreshes use SyncMembers and never emit welcomes.
func (manager *Manager) HandleMemberJoined(ctx context.Context, groupID int64) error {
	return manager.syncMembers(ctx, groupID, true)
}

func (manager *Manager) syncMembers(ctx context.Context, groupID int64, welcomeNew bool) error {
	lock := manager.groupLock(groupID)
	lock.Lock()
	defer lock.Unlock()
	roster, err := manager.Gateway.ListMembers(ctx, groupID)
	if err != nil {
		return err
	}
	remote := roster.Members
	local, err := manager.Store.ListAllMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return err
	}
	existing := make(map[int64]groupmgr.Member, len(local))
	existingByNIM := make(map[string]groupmgr.Member, len(local))
	for _, member := range local {
		existing[member.UserID] = member
		if member.NIMID != "" {
			existingByNIM[member.NIMID] = member
		}
	}
	settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, groupID)
	if err != nil {
		return err
	}
	initialSnapshot := settings.BaselineAt.IsZero()
	group, err := manager.group(ctx, groupID)
	if err != nil {
		return err
	}
	remoteIDs := make([]int64, 0, len(remote))
	newMembers := make([]groupmgr.Member, 0)
	selfID, _ := strconv.ParseInt(manager.AccountID, 10, 64)
	selfCanManage := false
	for _, member := range remote {
		if member.UserID == selfID && isManagerRole(member.Role) {
			selfCanManage = true
			break
		}
	}
	if !selfCanManage {
		group.ModerationEnabled = false
	}
	for _, member := range remote {
		previous, exists := existing[member.UserID]
		if !exists && member.NIMID != "" {
			if byNIM, found := existingByNIM[member.NIMID]; found {
				previous, exists = byNIM, true
				member.UserID = byNIM.UserID
			}
		}
		remoteIDs = append(remoteIDs, member.UserID)
		member.AccountID = manager.AccountID
		now := time.Now().UTC()
		member.Present, member.LastSeenAt, member.UpdatedAt = true, now, now
		newPresence := !exists || !previous.Present
		if newPresence {
			member.Blacklisted, _ = manager.Store.IsBlacklisted(ctx, manager.AccountID, groupID, member.UserID)
			member.OriginalCardName = member.CardName
			member.LockedCardName = member.CardName
			member.JoinedAt, member.DiscoveredAt = now, now
			member.CardStatus = groupmgr.CardUnmanaged
			switch {
			case initialSnapshot:
				member.JoinSource, member.NoticeRead = groupmgr.JoinBaseline, true
			case welcomeNew:
				member.JoinSource, member.NoticeRead = groupmgr.JoinOnline, false
			default:
				member.JoinSource, member.NoticeRead = groupmgr.JoinOffline, false
			}
		}
		if exists {
			member.Blacklisted = previous.Blacklisted
			member.OriginalCardName = previous.OriginalCardName
			member.ManagedCardName = previous.ManagedCardName
			member.CardSuffix = previous.CardSuffix
			member.CardStatus = previous.CardStatus
			member.JoinSource = previous.JoinSource
			member.NoticeRead = previous.NoticeRead
			member.LockedCardName = previous.LockedCardName
			member.RenameViolations = previous.RenameViolations
			member.JoinedAt = previous.JoinedAt
			member.DiscoveredAt = previous.DiscoveredAt
			if newPresence {
				member.JoinedAt = now
				if welcomeNew {
					member.JoinSource = groupmgr.JoinOnline
				} else {
					member.JoinSource = groupmgr.JoinOffline
				}
				member.NoticeRead = false
			}
			if member.LockedCardName == "" && member.CardName != "" {
				// Older snapshots may only contain IDs. The first resolved group
				// card becomes the immutable baseline, not a rename violation.
				member.LockedCardName = member.CardName
			}
			if previous.LockedCardName != "" && member.CardName != "" && member.CardName != previous.LockedCardName && previous.CardStatus != groupmgr.CardQueued && previous.CardStatus != groupmgr.CardRunning {
				member.RenameViolations++
				_ = manager.processNicknameChange(ctx, group, &member, previous.LockedCardName)
			}
		}
		if newPresence && !initialSnapshot {
			newMembers = append(newMembers, member)
		}
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			return err
		}
		if member.Blacklisted && selfCanManage && group.Enabled && group.ModerationEnabled && !manager.IsPaused() {
			_ = manager.execute(ctx, group, nil, &member, moderation.Action{Type: moderation.ActionRemove, Message: "blacklisted member snapshot"})
		}
	}
	if roster.Complete {
		if err := manager.Store.DeleteMembersExcept(ctx, manager.AccountID, groupID, remoteIDs); err != nil {
			return err
		}
	}
	now := time.Now().UTC()
	settings.AccountID, settings.GroupID = manager.AccountID, groupID
	settings.LastSnapshotAt = now
	settings.ReportedCount, settings.ResolvedCount, settings.RosterComplete = roster.ReportedCount, roster.ResolvedCount, roster.Complete
	if initialSnapshot && len(remote) > 0 {
		settings.BaselineAt = now
	}
	settings.UpdatedAt = now
	if err := manager.Store.UpsertCardSettings(ctx, settings); err != nil {
		return err
	}
	if group.Enabled && !selfCanManage {
		group.Enabled, group.AIEnabled, group.ModerationEnabled = false, false, false
		group.UpdatedAt = time.Now().UTC()
		if err := manager.Store.UpsertGroup(ctx, group); err != nil {
			return err
		}
		manager.audit(ctx, groupID, selfID, "manager_permission_lost", "warn", ErrManagerRequired.Error())
		enabledGroups, _ := manager.Store.ListGroups(ctx, manager.AccountID, true)
		manager.mu.Lock()
		manager.status.EnabledGroups = len(enabledGroups)
		manager.mu.Unlock()
	}
	if selfCanManage {
		_ = manager.Store.ResumePausedCardJobs(ctx, manager.AccountID, groupID)
	}
	if !initialSnapshot && selfCanManage && group.Enabled && settings.AutoRename {
		for _, member := range newMembers {
			_ = manager.queueAutoCardMember(ctx, group, settings, member, welcomeNew && member.JoinSource == groupmgr.JoinOnline)
		}
	}
	if welcomeNew && !initialSnapshot && selfCanManage && group.Enabled && !settings.AutoRename && group.WelcomeMessage != "" && !manager.IsPaused() {
		for _, member := range newMembers {
			name := resolvedName(member)
			if name == "" {
				manager.audit(ctx, groupID, member.UserID, "welcome_skipped", "warn", "成员名称尚未同步")
				continue
			}
			welcome := strings.ReplaceAll(group.WelcomeMessage, "@[成员]", "@「"+name+"」")
			welcome = strings.ReplaceAll(welcome, "[成员]", name)
			if _, err := manager.Gateway.SendText(ctx, groupID, welcome); err != nil {
				manager.audit(ctx, groupID, member.UserID, "welcome_error", "warn", err.Error())
				continue
			}
			manager.audit(ctx, groupID, member.UserID, "welcome", "info", welcome)
		}
	}
	return nil
}

func (manager *Manager) processNicknameChange(ctx context.Context, group groupmgr.Group, member *groupmgr.Member, lockedName string) error {
	if !group.Enabled || !group.ModerationEnabled || manager.IsPaused() {
		return nil
	}
	message := moderation.Message{
		AccountID: manager.AccountID, GroupID: group.GroupID, UserID: member.UserID,
		Kind: moderation.MessageOther, SentAt: time.Now().UTC(), Role: moderation.MemberRole(member.Role),
		NicknameChanged: true, OriginalNickname: lockedName, CurrentNickname: member.CardName,
	}
	rules, err := manager.Store.ListRules(ctx, group.GroupID)
	if err != nil {
		return err
	}
	manager.moderatorMu.Lock()
	defer manager.moderatorMu.Unlock()
	if err := manager.Moderator.SetRules(moderation.RulesFromModel(rules)); err != nil {
		return err
	}
	evaluation := manager.Moderator.Evaluate(message)
	for _, action := range evaluation.ExecutableActions {
		if action.Type == moderation.ActionRename && action.Nickname == "" {
			action.Nickname = lockedName
		}
		if err := manager.execute(ctx, group, nil, member, action); err != nil {
			continue
		}
		if action.Type == moderation.ActionRename {
			notice := fmt.Sprintf("@%s 你改你的名称了，已按原来的群名片恢复为：%s", displayName(*member), lockedName)
			if _, err := manager.Gateway.SendText(ctx, group.GroupID, notice); err == nil {
				manager.audit(ctx, group.GroupID, member.UserID, "nickname_restored", "info", notice)
			}
		}
	}
	return nil
}

func (manager *Manager) Process(ctx context.Context, incoming Incoming) (ProcessResult, error) {
	return manager.process(ctx, incoming, false)
}

func (manager *Manager) ProcessAsync(ctx context.Context, incoming Incoming) (ProcessResult, error) {
	return manager.process(ctx, incoming, true)
}

func (manager *Manager) process(ctx context.Context, incoming Incoming, asyncAI bool) (ProcessResult, error) {
	result := ProcessResult{MaxSequence: incoming.Sequence}
	lock := manager.groupLock(incoming.GroupID)
	lock.Lock()
	defer lock.Unlock()
	group, err := manager.group(ctx, incoming.GroupID)
	if errors.Is(err, groupmgr.ErrNotFound) {
		return result, nil
	}
	if err != nil {
		return result, err
	}
	if incoming.ServerMessageID == "" {
		incoming.ServerMessageID = fmt.Sprintf("seq-%d", incoming.Sequence)
	}
	message := groupmgr.Message{
		AccountID: manager.AccountID, GroupID: incoming.GroupID, ServerMessageID: incoming.ServerMessageID,
		Sequence: int64(incoming.Sequence), UserID: incoming.UserID, SenderName: incoming.SenderName,
		Kind: incoming.Kind, Text: incoming.Text, SentAt: incoming.SentAt, ReceivedAt: time.Now().UTC(),
	}
	inserted, err := manager.Store.InsertMessage(ctx, &message)
	if err != nil {
		return result, err
	}
	result.Stored = inserted
	result.MessageID = message.ID
	if !inserted {
		return result, nil
	}
	canManage, _, _ := manager.CanManageGroup(ctx, group.GroupID)
	if !group.Enabled || !canManage || manager.IsPaused() {
		_ = manager.Store.SetMessageProcessed(ctx, message.ID)
		manager.mu.Lock()
		manager.status.LastMessage = time.Now().UTC()
		manager.status.ProcessedCount++
		manager.mu.Unlock()
		return result, nil
	}
	member := manager.member(ctx, incoming.GroupID, incoming.UserID, incoming.SenderName)
	semanticEnabled := false
	if group.ModerationEnabled {
		rules, loadErr := manager.Store.ListRules(ctx, incoming.GroupID)
		if loadErr != nil {
			return result, loadErr
		}
		manager.moderatorMu.Lock()
		if loadErr = manager.Moderator.SetRules(moderation.RulesFromModel(rules)); loadErr != nil {
			manager.moderatorMu.Unlock()
			return result, loadErr
		}
		for _, rule := range rules {
			if rule.Enabled && rule.Matcher == groupmgr.MatcherSemantic {
				semanticEnabled = true
				break
			}
		}
		evaluation := manager.Moderator.Evaluate(moderation.MessageFromModel(message, member, nil))
		manager.moderatorMu.Unlock()
		for _, match := range evaluation.Matches {
			if match.Matched {
				result.RuleMatches++
			}
		}
		for _, action := range evaluation.ExecutableActions {
			if err := manager.execute(ctx, group, &message, member, action); err != nil {
				result.ProcessingErr = err.Error()
			}
			result.Actions++
		}
	}
	allowReply := group.AIEnabled && !group.ManualTakeover && aiprovider.ShouldReply(incoming.Text, manager.WakeWords)
	if incoming.Kind == groupmgr.MessageText && (allowReply || semanticEnabled) {
		if asyncAI {
			select {
			case manager.aiQueue <- aiJob{group: group, message: message, member: cloneMember(member), allowReply: allowReply}:
			default:
				manager.audit(ctx, group.GroupID, incoming.UserID, "ai_queue_full", "warn", "AI 任务队列已满（256 条）")
			}
		} else if err := manager.processAI(ctx, group, message, member, allowReply, &result); err != nil {
			result.ProcessingErr = err.Error()
			manager.audit(ctx, group.GroupID, incoming.UserID, "ai_error", "warn", err.Error())
		}
	}
	_ = manager.Store.SetMessageProcessed(ctx, message.ID)
	result.Handled = result.RuleMatches > 0 || result.AIReplied || result.CreatedTasks > 0
	manager.mu.Lock()
	manager.status.LastMessage = time.Now().UTC()
	manager.status.ProcessedCount++
	manager.mu.Unlock()
	return result, nil
}

func (manager *Manager) aiWorker() {
	for job := range manager.aiQueue {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		result := ProcessResult{}
		if err := manager.processAI(ctx, job.group, job.message, job.member, job.allowReply, &result); err != nil {
			manager.audit(ctx, job.group.GroupID, job.message.UserID, "ai_error", "warn", err.Error())
		}
		cancel()
	}
}

func cloneMember(member *groupmgr.Member) *groupmgr.Member {
	if member == nil {
		return nil
	}
	copy := *member
	return &copy
}

func (manager *Manager) processAI(ctx context.Context, group groupmgr.Group, message groupmgr.Message, member *groupmgr.Member, allowReply bool, result *ProcessResult) error {
	if manager.IsPaused() {
		return nil
	}
	manager.mu.RLock()
	provider := manager.Provider
	manager.mu.RUnlock()
	request := aiprovider.Request{
		Version: aiprovider.ContractVersion, EventID: message.ServerMessageID, Persona: aiprovider.DefaultPersona,
		Group:   aiprovider.Group{ID: group.GroupID, Name: group.Name},
		Member:  aiprovider.Member{UserID: message.UserID, Name: message.SenderName},
		Message: aiprovider.Message{IDServer: message.ServerMessageID, Format: string(message.Kind), Text: message.Text, Time: message.SentAt.UnixMilli()},
	}
	recent, _ := manager.Store.ListRecentMessages(ctx, manager.AccountID, group.GroupID, 20)
	var predictionPrompt string
	var predictionSnapshot prediction.Snapshot
	var predictionErr error
	if prediction.IsRequest(message.Text) && manager.Prediction != nil {
		predictionSnapshot, predictionPrompt, predictionErr = manager.Prediction.Build(ctx, message.Text)
		if predictionErr != nil {
			request.Message.Text += "\n\n预测数据状态：暂时不可用，请说明数据暂不可用，不要编造结果。"
		} else if predictionSnapshot.Game.ID == "" {
			if allowReply {
				if _, err := manager.Gateway.SendText(ctx, group.GroupID, predictionPrompt); err != nil {
					return err
				}
				result.AIReplied = true
			}
			return nil
		} else {
			request.Message.Text += "\n\n" + predictionPrompt
			if len(predictionSnapshot.Result) == 0 {
				request.Message.Text += "\n预测结果为空时请直接说明数据状态。"
			}
		}
	}
	if provider == nil {
		if allowReply && predictionSnapshot.Game.ID != "" {
			if _, err := manager.Gateway.SendText(ctx, group.GroupID, prediction.Fallback(predictionSnapshot)); err != nil {
				return err
			}
			result.AIReplied = true
		} else if allowReply && prediction.IsRequest(message.Text) && predictionErr != nil {
			if _, err := manager.Gateway.SendText(ctx, group.GroupID, "预测数据暂不可用，请稍后再试；当前不会补造结果。"); err != nil {
				return err
			}
			result.AIReplied = true
		}
		return nil
	}
	for index := len(recent) - 1; index >= 0; index-- {
		candidate := recent[index]
		if candidate.ID == message.ID || strings.TrimSpace(candidate.Text) == "" {
			continue
		}
		request.RecentContext = append(request.RecentContext, aiprovider.ContextMessage{
			UserID: candidate.UserID, Name: candidate.SenderName, Text: candidate.Text, Time: candidate.SentAt.UnixMilli(),
		})
	}
	if member != nil {
		request.Member.Name, request.Member.Role = displayName(*member), string(member.Role)
	}
	items, err := manager.Store.ListKnowledgeForGroup(ctx, manager.AccountID, group.GroupID)
	if err != nil {
		return err
	}
	var index knowledge.Index
	baseNames := make(map[string]string, len(items))
	for _, item := range items {
		documentID := fmt.Sprintf("%d", item.ID)
		baseNames[documentID] = item.BaseName
		index.Add(knowledge.Document{ID: documentID, GroupID: group.GroupID, Title: item.Title, Source: item.Source, Text: item.Content})
	}
	seenKnowledge := make(map[string]bool)
	for _, candidate := range index.Search(group.GroupID, message.Text, 6) {
		if seenKnowledge[candidate.Chunk.DocumentID] {
			continue
		}
		seenKnowledge[candidate.Chunk.DocumentID] = true
		request.Knowledge = append(request.Knowledge, aiprovider.KnowledgeChunk{Source: candidate.Chunk.Source, Title: candidate.Chunk.Title, Base: baseNames[candidate.Chunk.DocumentID], Text: candidate.Chunk.Text})
	}
	decision, err := provider.Decide(ctx, request)
	if err != nil {
		if allowReply && predictionSnapshot.Game.ID != "" {
			if _, sendErr := manager.Gateway.SendText(ctx, group.GroupID, prediction.Fallback(predictionSnapshot)); sendErr != nil {
				return sendErr
			}
			result.AIReplied = true
			manager.audit(ctx, group.GroupID, message.UserID, "prediction_fallback", "warn", "AI 表达暂不可用，已发送本地统计结果")
			return nil
		}
		return err
	}
	if allowReply && strings.TrimSpace(decision.Reply) != "" {
		if _, err := manager.Gateway.SendText(ctx, group.GroupID, decision.Reply); err != nil {
			return err
		}
		result.AIReplied = true
	}
	semanticScores := make(map[string]float64)
	for _, proposed := range decision.Actions {
		if proposed.Category != "" {
			score := proposed.Confidence
			if score == 0 {
				score = decision.Confidence
			}
			if score > semanticScores[proposed.Category] {
				semanticScores[proposed.Category] = score
			}
		}
		details, _ := json.Marshal(proposed)
		_, _ = manager.Store.RecordAction(ctx, groupmgr.ActionRecord{
			AccountID: manager.AccountID, GroupID: group.GroupID, UserID: proposed.TargetUserID,
			MessageID: message.ID, Type: groupmgr.ActionType(proposed.Type), Mode: groupmgr.RuleObserve,
			Reason: string(details), Success: true, CreatedAt: time.Now().UTC(),
		})
	}
	if len(semanticScores) > 0 && group.ModerationEnabled {
		if err := manager.applySemanticDecision(ctx, group, message, member, semanticScores, result); err != nil {
			return err
		}
	}
	for _, proposed := range decision.Tasks {
		task := groupmgr.Task{GroupID: group.GroupID, AssigneeID: proposed.AssigneeID, CreatedByID: message.UserID, Title: proposed.Title, Description: proposed.Description, Status: groupmgr.TaskPending, CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC()}
		if proposed.DueAt > 0 {
			due := time.UnixMilli(proposed.DueAt).UTC()
			task.DueAt = &due
		}
		if err := manager.Store.UpsertTask(ctx, &task); err == nil {
			result.CreatedTasks++
		}
	}
	manager.audit(ctx, group.GroupID, message.UserID, "ai_decision", "info", decision.Reason)
	return nil
}

func (manager *Manager) applySemanticDecision(ctx context.Context, group groupmgr.Group, message groupmgr.Message, member *groupmgr.Member, scores map[string]float64, result *ProcessResult) error {
	storedRules, err := manager.Store.ListRules(ctx, group.GroupID)
	if err != nil {
		return err
	}
	semanticRules := make([]groupmgr.ModerationRule, 0)
	for _, rule := range storedRules {
		if rule.Enabled && rule.Matcher == groupmgr.MatcherSemantic {
			semanticRules = append(semanticRules, rule)
		}
	}
	if len(semanticRules) == 0 {
		return nil
	}
	manager.moderatorMu.Lock()
	if err := manager.Moderator.SetRules(moderation.RulesFromModel(semanticRules)); err != nil {
		manager.moderatorMu.Unlock()
		return err
	}
	evaluation := manager.Moderator.Evaluate(moderation.MessageFromModel(message, member, scores))
	manager.moderatorMu.Unlock()
	for _, match := range evaluation.Matches {
		if !match.Matched {
			continue
		}
		result.RuleMatches++
		if match.Rule.Mode == moderation.ModeAuto && !match.Suppressed {
			continue
		}
		for _, action := range match.Actions {
			_, _ = manager.Store.RecordAction(ctx, groupmgr.ActionRecord{
				AccountID: manager.AccountID, GroupID: group.GroupID, UserID: message.UserID,
				MessageID: message.ID, RuleID: match.Rule.ID, Type: groupmgr.ActionType(action.Type),
				Mode: groupmgr.RuleMode(match.Rule.Mode), Reason: match.Reason, Success: true, CreatedAt: time.Now().UTC(),
			})
		}
	}
	for _, action := range evaluation.ExecutableActions {
		if err := manager.execute(ctx, group, &message, member, action); err != nil {
			result.ProcessingErr = err.Error()
		}
		result.Actions++
	}
	return nil
}

func (manager *Manager) execute(ctx context.Context, group groupmgr.Group, message *groupmgr.Message, member *groupmgr.Member, action moderation.Action) error {
	userID := int64(0)
	messageID := int64(0)
	serverMessageID := ""
	if member != nil {
		userID = member.UserID
	}
	if message != nil {
		messageID, serverMessageID = message.ID, message.ServerMessageID
		if userID == 0 {
			userID = message.UserID
		}
	}
	var err error
	switch action.Type {
	case moderation.ActionRecall:
		err = manager.Gateway.Recall(ctx, group.GroupID, userID, serverMessageID)
	case moderation.ActionMute:
		err = manager.Gateway.Mute(ctx, group.GroupID, userID, action.Duration)
	case moderation.ActionRemove:
		err = manager.Gateway.RemoveMember(ctx, group.GroupID, userID)
	case moderation.ActionRename:
		ref := groupmgr.MemberRef{UserID: userID}
		if member != nil {
			ref.NIMID = member.NIMID
		}
		err = manager.Gateway.Rename(ctx, group.GroupID, ref, action.Nickname)
	case moderation.ActionBlacklist:
		if member != nil {
			member.Blacklisted = true
			member.UpdatedAt = time.Now().UTC()
			err = manager.Store.UpsertMember(ctx, *member)
		}
	case moderation.ActionReply:
		_, err = manager.Gateway.SendText(ctx, group.GroupID, action.Reply)
	case moderation.ActionNotify:
		if action.Message != "" {
			_, err = manager.Gateway.SendText(ctx, group.GroupID, action.Message)
		}
	}
	record := groupmgr.ActionRecord{
		AccountID: manager.AccountID, GroupID: group.GroupID, UserID: userID, MessageID: messageID,
		RuleID: action.RuleID, Type: groupmgr.ActionType(action.Type), Mode: groupmgr.RuleAuto,
		Duration: action.Duration, Reason: action.Message, Success: err == nil, CreatedAt: time.Now().UTC(),
	}
	if err != nil {
		record.Error = err.Error()
	}
	_, _ = manager.Store.RecordAction(ctx, record)
	manager.mu.Lock()
	manager.status.ActionCount++
	manager.mu.Unlock()
	return err
}

func (manager *Manager) SendManual(ctx context.Context, groupID int64, text string) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	if strings.TrimSpace(text) == "" {
		return errors.New("消息内容为空")
	}
	_, err := manager.Gateway.SendText(ctx, groupID, text)
	if err == nil {
		manager.audit(ctx, groupID, 0, "manual_reply", "info", text)
	}
	return err
}

func (manager *Manager) MuteMember(ctx context.Context, groupID, userID int64, duration time.Duration) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	err := manager.Gateway.Mute(ctx, groupID, userID, duration)
	manager.recordManualAction(ctx, groupID, userID, groupmgr.ActionMute, duration, err)
	return err
}

func (manager *Manager) UnmuteMember(ctx context.Context, groupID, userID int64) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	err := manager.Gateway.Unmute(ctx, groupID, userID)
	manager.recordManualAction(ctx, groupID, userID, groupmgr.ActionUnmute, 0, err)
	return err
}

func (manager *Manager) RenameMember(ctx context.Context, groupID, userID int64, nickname string) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	member, _ := manager.Store.GetMember(ctx, manager.AccountID, groupID, userID)
	err := manager.Gateway.Rename(ctx, groupID, groupmgr.MemberRef{UserID: userID, NIMID: member.NIMID}, nickname)
	manager.recordManualAction(ctx, groupID, userID, groupmgr.ActionRename, 0, err)
	return err
}

func (manager *Manager) RemoveMember(ctx context.Context, groupID, userID int64) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	err := manager.Gateway.RemoveMember(ctx, groupID, userID)
	manager.recordManualAction(ctx, groupID, userID, groupmgr.ActionRemove, 0, err)
	return err
}

func (manager *Manager) SetGroupMute(ctx context.Context, groupID int64, muted bool) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	err := manager.Gateway.SetGroupMute(ctx, groupID, muted)
	reason := "off"
	if muted {
		reason = "on"
	}
	manager.recordAction(ctx, groupmgr.ActionRecord{
		AccountID: manager.AccountID, GroupID: groupID, Type: groupmgr.ActionGroupMute,
		Mode: groupmgr.RuleAuto, Reason: reason, Success: err == nil, Error: errorText(err), CreatedAt: time.Now().UTC(),
	})
	return err
}

func (manager *Manager) recordManualAction(ctx context.Context, groupID, userID int64, actionType groupmgr.ActionType, duration time.Duration, err error) {
	manager.recordAction(ctx, groupmgr.ActionRecord{
		AccountID: manager.AccountID, GroupID: groupID, UserID: userID, Type: actionType,
		Mode: groupmgr.RuleAuto, Duration: duration, Reason: "manual", Success: err == nil,
		Error: errorText(err), CreatedAt: time.Now().UTC(),
	})
}

func (manager *Manager) recordAction(ctx context.Context, record groupmgr.ActionRecord) {
	_, _ = manager.Store.RecordAction(ctx, record)
	manager.mu.Lock()
	manager.status.ActionCount++
	manager.mu.Unlock()
	level := "info"
	if !record.Success {
		level = "warn"
	}
	manager.audit(ctx, record.GroupID, record.UserID, "manual_"+string(record.Type), level, firstNonEmpty(record.Error, record.Reason))
}

func (manager *Manager) TestAI(ctx context.Context, groupID int64) (string, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return "", err
	}
	group, err := manager.group(ctx, groupID)
	if err != nil {
		return "", err
	}
	manager.mu.RLock()
	provider := manager.Provider
	manager.mu.RUnlock()
	if provider == nil {
		return "", errors.New("AI 服务尚未配置")
	}
	decision, err := provider.Decide(ctx, aiprovider.Request{
		Version: aiprovider.ContractVersion, EventID: fmt.Sprintf("test-%d", time.Now().UnixNano()),
		Group: groupToAI(group), Member: aiprovider.Member{UserID: 1, Name: "DH", Role: "owner"},
		Message: aiprovider.Message{Format: "test", Text: "DH connection test", Time: time.Now().UnixMilli()},
	})
	if err != nil {
		return "", err
	}
	manager.audit(ctx, groupID, 0, "ai_test", "info", decision.Reason)
	return firstNonEmpty(decision.Reply, "AI 响应有效"), nil
}

func (manager *Manager) RunScheduled(ctx context.Context, now time.Time) error {
	if manager.IsPaused() {
		return nil
	}
	if now.IsZero() {
		now = time.Now()
	}
	groups, err := manager.Store.ListGroups(ctx, manager.AccountID, true)
	if err != nil {
		return err
	}
	var lastErr error
	if err := manager.runGroupSchedules(ctx, now); err != nil {
		lastErr = err
	}
	for _, group := range groups {
		if err := manager.RequireManager(ctx, group.GroupID); err != nil {
			continue
		}
		if group.TaskReminders {
			if err := manager.remindDueTasks(ctx, group, now); err != nil {
				lastErr = err
			}
		}
		if !group.AIEnabled || group.ManualTakeover || !summaryDue(group.SummarySchedule, now) {
			continue
		}
		key := fmt.Sprintf("summary.last.%s.%d", manager.AccountID, group.GroupID)
		date := now.Local().Format("2006-01-02")
		if previous, err := manager.Store.GetSetting(ctx, key); err == nil && previous.Value == date {
			continue
		}
		if _, err := manager.summarize(ctx, group.GroupID, "scheduled"); err != nil {
			manager.audit(ctx, group.GroupID, 0, "summary_error", "warn", err.Error())
			lastErr = err
			continue
		}
		_ = manager.Store.SetSetting(ctx, groupmgr.AppSetting{Key: key, Value: date, UpdatedAt: now.UTC()})
	}
	return lastErr
}

// runGroupSchedules applies the most recent daily speaking-state transition.
// The unique run key makes startup catch-up and the 30-second poll idempotent.
func (manager *Manager) runGroupSchedules(ctx context.Context, now time.Time) error {
	schedules, err := manager.Store.ListGroupSchedules(ctx, manager.AccountID)
	if err != nil {
		return err
	}
	local := now.Local()
	var lastErr error
	for _, schedule := range schedules {
		if !schedule.Enabled || len(schedule.GroupIDs) == 0 {
			continue
		}
		action, due, localDate := scheduledAction(schedule, local)
		if !due {
			continue
		}
		for _, groupID := range schedule.GroupIDs {
			if err := manager.RequireManager(ctx, groupID); err != nil {
				lastErr = err
				continue
			}
			runKey := fmt.Sprintf("%s:%d:%d:%s:%s", manager.AccountID, schedule.ID, groupID, localDate, action)
			inserted, err := manager.Store.RecordScheduleRun(ctx, &groupmgr.ScheduleRun{ScheduleID: schedule.ID, AccountID: manager.AccountID, GroupID: groupID, LocalDate: localDate, Action: action, RunKey: runKey, CreatedAt: now.UTC()})
			if err != nil || !inserted {
				if err != nil {
					lastErr = err
				}
				continue
			}
			muted := action == groupmgr.ActionGroupMute
			err = manager.SetGroupMute(ctx, groupID, muted)
			_ = manager.Store.UpdateScheduleRun(ctx, runKey, err == nil, errorText(err))
			if err != nil {
				lastErr = err
				manager.audit(ctx, groupID, 0, "group_schedule_error", "warn", userFacingScheduleError(err))
			} else {
				manager.audit(ctx, groupID, 0, "group_schedule_"+string(action), "info", schedule.Name)
			}
		}
	}
	return lastErr
}

func scheduledAction(schedule groupmgr.GroupSchedule, now time.Time) (groupmgr.ActionType, bool, string) {
	open, openErr := time.ParseInLocation("15:04", schedule.OpenTime, now.Location())
	close, closeErr := time.ParseInLocation("15:04", schedule.CloseTime, now.Location())
	if openErr != nil || closeErr != nil || schedule.OpenTime == schedule.CloseTime {
		return "", false, ""
	}
	date := now.Format("2006-01-02")
	openAt := time.Date(now.Year(), now.Month(), now.Day(), open.Hour(), open.Minute(), 0, 0, now.Location())
	closeAt := time.Date(now.Year(), now.Month(), now.Day(), close.Hour(), close.Minute(), 0, 0, now.Location())
	if closeAt.After(openAt) {
		if !now.Before(closeAt) {
			return groupmgr.ActionGroupMute, true, date
		}
		if !now.Before(openAt) {
			return groupmgr.ActionUnmute, true, date
		}
		return groupmgr.ActionGroupMute, true, now.AddDate(0, 0, -1).Format("2006-01-02")
	}
	// A close time earlier than the open time crosses midnight.
	if !now.Before(openAt) {
		return groupmgr.ActionUnmute, true, date
	}
	if now.Before(closeAt) {
		return groupmgr.ActionUnmute, true, now.AddDate(0, 0, -1).Format("2006-01-02")
	}
	return groupmgr.ActionGroupMute, true, date
}

func userFacingScheduleError(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func (manager *Manager) remindDueTasks(ctx context.Context, group groupmgr.Group, now time.Time) error {
	tasks, err := manager.Store.ListTasks(ctx, group.GroupID, groupmgr.TaskPending)
	if err != nil {
		return err
	}
	var lastErr error
	for index := range tasks {
		task := &tasks[index]
		if task.DueAt == nil || task.DueAt.After(now) || task.RemindedAt != nil {
			continue
		}
		assignee := ""
		if task.AssigneeID > 0 {
			if member, err := manager.Store.GetMember(ctx, manager.AccountID, group.GroupID, task.AssigneeID); err == nil {
				if name := resolvedName(member); name != "" {
					assignee = " @" + name
				}
			}
		}
		text := fmt.Sprintf("任务提醒%s：%s", assignee, task.Title)
		if _, err := manager.Gateway.SendText(ctx, group.GroupID, text); err != nil {
			manager.audit(ctx, group.GroupID, task.AssigneeID, "task_reminder_error", "warn", err.Error())
			lastErr = err
			continue
		}
		remindedAt := now.UTC()
		task.RemindedAt, task.UpdatedAt = &remindedAt, remindedAt
		if err := manager.Store.UpsertTask(ctx, task); err != nil {
			lastErr = err
			continue
		}
		manager.audit(ctx, group.GroupID, task.AssigneeID, "task_reminder", "info", task.Title)
	}
	return lastErr
}

func summaryDue(schedule string, now time.Time) bool {
	parsed, err := time.Parse("15:04", strings.TrimSpace(schedule))
	if err != nil {
		return false
	}
	local := now.Local()
	due := time.Date(local.Year(), local.Month(), local.Day(), parsed.Hour(), parsed.Minute(), 0, 0, local.Location())
	return !local.Before(due)
}

func (manager *Manager) Summarize(ctx context.Context, groupID int64) (string, error) {
	return manager.summarize(ctx, groupID, "manual")
}

func (manager *Manager) summarize(ctx context.Context, groupID int64, source string) (string, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return "", err
	}
	group, err := manager.group(ctx, groupID)
	if err != nil {
		return "", err
	}
	manager.mu.RLock()
	provider := manager.Provider
	manager.mu.RUnlock()
	if provider == nil {
		return "", errors.New("AI 服务尚未配置")
	}
	messages, err := manager.Store.ListRecentMessages(ctx, manager.AccountID, groupID, 500)
	if err != nil {
		return "", err
	}
	if len(messages) == 0 {
		return "", errors.New("当前群暂无可供摘要的消息")
	}
	request := aiprovider.Request{
		Version: aiprovider.ContractVersion, EventID: fmt.Sprintf("summary-%d-%d", groupID, time.Now().Unix()),
		Group: groupToAI(group), Member: aiprovider.Member{UserID: 1, Name: "DH管理员", Role: "owner"},
		Message: aiprovider.Message{Format: "summary", Text: "请总结最近群聊，列出结论、待办和负责人。", Time: time.Now().UnixMilli()},
	}
	for index := len(messages) - 1; index >= 0; index-- {
		message := messages[index]
		if strings.TrimSpace(message.Text) == "" {
			continue
		}
		request.RecentContext = append(request.RecentContext, aiprovider.ContextMessage{UserID: message.UserID, Name: message.SenderName, Text: message.Text, Time: message.SentAt.UnixMilli()})
	}
	decision, err := provider.Decide(ctx, request)
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(decision.Reply) == "" {
		return "", errors.New("AI 未返回摘要内容")
	}
	summary := groupmgr.DailySummary{AccountID: manager.AccountID, GroupID: groupID, Content: strings.TrimSpace(decision.Reply), Source: source, CreatedAt: time.Now().UTC()}
	if err := manager.Store.SaveDailySummary(ctx, &summary); err != nil {
		return "", err
	}
	manager.audit(ctx, groupID, 0, "summary", "info", fmt.Sprintf("本地摘要 #%d", summary.ID))
	return summary.Content, nil
}

func groupToAI(group groupmgr.Group) aiprovider.Group {
	return aiprovider.Group{ID: group.GroupID, Name: group.Name}
}

func errorText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}

func (manager *Manager) group(ctx context.Context, groupID int64) (groupmgr.Group, error) {
	groups, err := manager.Store.ListGroups(ctx, manager.AccountID, false)
	if err != nil {
		return groupmgr.Group{}, err
	}
	for _, group := range groups {
		if group.GroupID == groupID {
			return group, nil
		}
	}
	return groupmgr.Group{}, groupmgr.ErrNotFound
}

func (manager *Manager) member(ctx context.Context, groupID, userID int64, name string) *groupmgr.Member {
	members, _ := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	for index := range members {
		if members[index].UserID == userID {
			return &members[index]
		}
	}
	now := time.Now().UTC()
	member := groupmgr.Member{AccountID: manager.AccountID, GroupID: groupID, UserID: userID, Nickname: name, CardName: name, OriginalCardName: name, LockedCardName: name, Role: groupmgr.RoleMember, Present: true, CardStatus: groupmgr.CardUnmanaged, JoinSource: groupmgr.JoinMessage, NoticeRead: false, JoinedAt: now, DiscoveredAt: now, LastSeenAt: now, UpdatedAt: now}
	_ = manager.Store.UpsertMember(ctx, member)
	if group, err := manager.group(ctx, groupID); err == nil && group.Enabled {
		if settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, groupID); err == nil && settings.AutoRename {
			_ = manager.queueAutoCardMember(ctx, group, settings, member, false)
		}
	}
	return &member
}

func (manager *Manager) groupLock(groupID int64) *sync.Mutex {
	manager.mu.Lock()
	defer manager.mu.Unlock()
	lock := manager.groupLocks[groupID]
	if lock == nil {
		lock = &sync.Mutex{}
		manager.groupLocks[groupID] = lock
	}
	return lock
}

func (manager *Manager) audit(ctx context.Context, groupID, userID int64, event, level, details string) {
	_, _ = manager.Store.RecordAudit(ctx, groupmgr.AuditEvent{AccountID: manager.AccountID, GroupID: groupID, UserID: userID, Actor: "DH", Event: event, Level: level, Details: details, CreatedAt: time.Now().UTC()})
}

func displayName(member groupmgr.Member) string {
	if name := resolvedName(member); name != "" {
		return name
	}
	return "成员"
}

func resolvedName(member groupmgr.Member) string {
	if strings.TrimSpace(member.CardName) != "" {
		return member.CardName
	}
	if strings.TrimSpace(member.Nickname) != "" {
		return member.Nickname
	}
	return ""
}
