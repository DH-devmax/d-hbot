package moderation

import (
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode"
)

const DefaultMuteDuration = 10 * time.Minute

type historyEvent struct {
	at              time.Time
	accountID       string
	groupID         int64
	userID          int64
	kind            MessageKind
	nicknameChanged bool
}

type lifetimeCounts struct {
	images  int
	renames int
}

// Engine is safe for concurrent message listeners. Evaluation updates only
// bounded per-user history and cooldown state; rules can be replaced atomically.
type Engine struct {
	mu       sync.Mutex
	rules    []ModerationRule
	regex    map[string]*regexp.Regexp
	history  map[string][]historyEvent
	counts   map[string]lifetimeCounts
	lastFire map[string]time.Time
	now      func() time.Time
}

// NewEngine returns an engine ready to use. Supplying a static rule set is
// optional; invalid static rules panic because they represent a programming
// error. Runtime/UI rule updates should use SetRules and handle its error.
func NewEngine(ruleSets ...[]ModerationRule) *Engine {
	e := &Engine{
		history:  make(map[string][]historyEvent),
		counts:   make(map[string]lifetimeCounts),
		lastFire: make(map[string]time.Time),
		now:      func() time.Time { return time.Now().UTC() },
	}
	if len(ruleSets) > 0 {
		if err := e.setRulesLocked(ruleSets[0]); err != nil {
			panic(err)
		}
	}
	return e
}

// NewValidatedEngine validates a runtime-provided rule set without panicking.
func NewValidatedEngine(rules []ModerationRule) (*Engine, error) {
	e := NewEngine()
	if err := e.SetRules(rules); err != nil {
		return nil, err
	}
	return e, nil
}

// MustNewEngine is retained as a descriptive constructor for static rules.
func MustNewEngine(rules []ModerationRule) *Engine {
	return NewEngine(rules)
}

// SetRules atomically replaces the active rules. Existing message history is
// retained so count windows and cooldowns survive UI edits/reloads.
func (e *Engine) SetRules(rules []ModerationRule) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.setRulesLocked(rules)
}

func (e *Engine) setRulesLocked(rules []ModerationRule) error {
	rules = SortRules(rules)
	regex := make(map[string]*regexp.Regexp)
	for i := range rules {
		r := &rules[i]
		if r.Mode == "" {
			r.Mode = ModeAuto
		}
		if err := ValidateRule(*r); err != nil {
			return err
		}
		if r.Matcher == MatcherRegex {
			rx, err := regexp.Compile(r.Pattern)
			if err != nil {
				return fmt.Errorf("规则 %d（%s）的正则表达式无效：%w", r.ID, r.Name, err)
			}
			regex[ruleIdentity(*r, i)] = rx
		}
		if r.Matcher == MatcherSemantic && r.SemanticThreshold == 0 && r.Threshold > 0 {
			r.SemanticThreshold = float64(r.Threshold) / 100
		}
		for j := range r.Actions {
			if r.Actions[j].Type == ActionMute && r.Actions[j].Duration <= 0 {
				r.Actions[j].Duration = DefaultMuteDuration
			}
		}
	}
	e.rules = append([]ModerationRule(nil), rules...)
	e.regex = regex
	return nil
}

// Rules returns a copy of the currently active configuration.
func (e *Engine) Rules() []ModerationRule {
	e.mu.Lock()
	defer e.mu.Unlock()
	return cloneRules(e.rules)
}

// EvaluateMessage is an explicit alias used by message processors.
func (e *Engine) EvaluateMessage(message Message) Evaluation { return e.Evaluate(message) }

// Evaluate matches all enabled rules applicable to message, then merges their
// actions deterministically. The returned Actions are useful for audit and
// dry-run previews; ExecutableActions contains only unsuppressed auto actions.
func (e *Engine) Evaluate(message Message, ruleSets ...[]ModerationRule) Evaluation {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(ruleSets) > 0 {
		if err := e.setRulesLocked(ruleSets[0]); err != nil {
			return Evaluation{Message: message, Error: err.Error()}
		}
	}
	now := message.SentAt
	if now.IsZero() {
		now = e.now()
		message.SentAt = now
	} else {
		now = now.UTC()
		message.SentAt = now
	}
	e.record(message, now)

	result := Evaluation{Message: message}
	var allActions, executable []Action
	for i, rule := range e.rules {
		if !rule.Enabled || (rule.GroupID != 0 && rule.GroupID != message.GroupID) {
			continue
		}
		match := RuleMatch{Rule: rule}
		if isExempt(rule, message) {
			match.Suppressed = true
			match.Reason = "exempt"
			result.Matches = append(result.Matches, match)
			continue
		}
		matched, reason := e.matches(rule, i, message, now)
		if !matched {
			match.Reason = reason
			result.Matches = append(result.Matches, match)
			continue
		}
		match.Matched = true
		match.Reason = reason
		match.Actions = ruleActions(rule, message)
		allActions = append(allActions, match.Actions...)
		if rule.Mode != ModeAuto {
			match.Suppressed = true
			match.Reason = reason + "; mode=" + string(rule.Mode)
		} else if e.inCooldown(rule, i, message, now) {
			match.Suppressed = true
			match.Reason = reason + "; cooldown"
		} else {
			executable = append(executable, match.Actions...)
			if rule.Cooldown > 0 {
				e.lastFire[cooldownKey(rule, i, message)] = now
			}
		}
		result.Matches = append(result.Matches, match)
	}
	result.Actions = mergeActions(allActions)
	result.ExecutableActions = mergeActions(executable)
	e.pruneHistory(now)
	return result
}

func (e *Engine) matches(rule ModerationRule, index int, message Message, now time.Time) (bool, string) {
	switch rule.Matcher {
	case MatcherExact:
		return message.Text == rule.Pattern, "exact"
	case MatcherContains:
		return strings.Contains(message.Text, rule.Pattern), "contains"
	case MatcherPrefix:
		return strings.HasPrefix(message.Text, rule.Pattern), "prefix"
	case MatcherRegex:
		rx := e.regex[ruleIdentity(rule, index)]
		if rx == nil {
			return false, "regex unavailable"
		}
		return rx.MatchString(message.Text), "regex"
	case MatcherLength:
		return weightedLength(message.Text) > effectiveThreshold(rule), "length"
	case MatcherLines:
		return lineCount(message.Text) > effectiveThreshold(rule), "lines"
	case MatcherImageCount:
		if message.Kind != MessageImage {
			return false, "not image"
		}
		count := e.countSince(message, now, rule.Window, MatcherImageCount, func(event historyEvent) bool { return event.kind == MessageImage })
		return count >= effectiveCount(rule), fmt.Sprintf("image_count=%d", count)
	case MatcherSemantic:
		score := message.SemanticScore
		if message.SemanticScores != nil {
			if candidate, ok := message.SemanticScores[rule.Pattern]; ok {
				score = candidate
			}
		}
		threshold := rule.SemanticThreshold
		if threshold <= 0 {
			threshold = float64(effectiveThreshold(rule)) / 100
		}
		return score >= threshold, fmt.Sprintf("semantic=%.3f", score)
	case MatcherRenameCount:
		if !message.NicknameChanged {
			return false, "nickname unchanged"
		}
		count := e.countSince(message, now, rule.Window, MatcherRenameCount, func(event historyEvent) bool { return event.nicknameChanged })
		return count >= effectiveCount(rule), fmt.Sprintf("rename_count=%d", count)
	case MatcherBlacklist:
		return message.Blacklisted, "blacklist"
	default:
		return false, "unknown matcher"
	}
}

func (e *Engine) record(message Message, now time.Time) {
	key := subjectKey(message)
	counts := e.counts[key]
	if message.Kind == MessageImage {
		counts.images++
	}
	if message.NicknameChanged {
		counts.renames++
	}
	e.counts[key] = counts
	e.history[key] = append(e.history[key], historyEvent{
		at: now, accountID: message.AccountID, groupID: message.GroupID, userID: message.UserID,
		kind: message.Kind, nicknameChanged: message.NicknameChanged,
	})
}

func (e *Engine) countSince(message Message, now time.Time, window time.Duration, matcher MatcherType, predicate func(historyEvent) bool) int {
	if window <= 0 {
		counts := e.counts[subjectKey(message)]
		if matcher == MatcherImageCount {
			return counts.images
		}
		if matcher == MatcherRenameCount {
			persisted := message.PriorRenameCount
			if message.NicknameChanged {
				persisted++
			}
			if persisted > counts.renames {
				return persisted
			}
			return counts.renames
		}
	}
	count := 0
	for _, event := range e.history[subjectKey(message)] {
		if window > 0 && !withinWindow(now, event.at, window) {
			continue
		}
		if predicate(event) {
			count++
		}
	}
	return count
}

func (e *Engine) pruneHistory(now time.Time) {
	maxWindow := time.Duration(0)
	for _, rule := range e.rules {
		if rule.Window > maxWindow {
			maxWindow = rule.Window
		}
	}
	if maxWindow <= 0 {
		return
	}
	for key, events := range e.history {
		keep := events[:0]
		for _, event := range events {
			if withinWindow(now, event.at, maxWindow) || event.at.After(now) {
				keep = append(keep, event)
			}
		}
		if len(keep) == 0 {
			delete(e.history, key)
		} else {
			e.history[key] = keep
		}
	}
}

func (e *Engine) inCooldown(rule ModerationRule, index int, message Message, now time.Time) bool {
	if rule.Cooldown <= 0 {
		return false
	}
	last, ok := e.lastFire[cooldownKey(rule, index, message)]
	return ok && now.Sub(last) < rule.Cooldown
}

func isExempt(rule ModerationRule, message Message) bool {
	for _, role := range rule.ExemptRoles {
		if role == message.Role {
			return true
		}
	}
	for _, userID := range rule.ExemptUserIDs {
		if userID == message.UserID {
			return true
		}
	}
	return false
}

func ruleActions(rule ModerationRule, message Message) []Action {
	actions := make([]Action, 0, len(rule.Actions))
	for _, configured := range rule.Actions {
		if configured.Type == "" {
			continue
		}
		nickname := configured.Nickname
		if configured.Type == ActionRename && nickname == "" {
			nickname = message.OriginalNickname
		}
		actions = append(actions, Action{
			Type: configured.Type, Duration: configured.Duration, Reply: configured.Reply,
			Nickname: nickname, Message: configured.Message,
			RuleID: rule.ID, Priority: rule.Priority,
		})
	}
	return actions
}

func mergeActions(actions []Action) []Action {
	if len(actions) == 0 {
		return nil
	}
	var recall, blacklist bool
	var remove, mute, reply, rename *Action
	notifies := make([]Action, 0)
	seenNotify := make(map[string]bool)
	for i := range actions {
		action := actions[i]
		switch action.Type {
		case ActionRecall:
			recall = true
		case ActionBlacklist:
			blacklist = true
		case ActionRemove:
			remove = stronger(remove, action)
		case ActionMute:
			mute = stronger(mute, action)
		case ActionReply:
			reply = stronger(reply, action)
		case ActionRename:
			rename = stronger(rename, action)
		case ActionNotify:
			key := action.Message
			if key == "" {
				key = action.Reply
			}
			if !seenNotify[key] {
				seenNotify[key] = true
				notifies = append(notifies, action)
			}
		}
	}
	out := make([]Action, 0, 1+len(notifies)+3)
	if recall {
		out = append(out, Action{Type: ActionRecall})
	}
	if rename != nil && remove == nil {
		out = append(out, *rename)
	}
	// remove > mute > reply: lower-strength actions do not survive a stronger
	// disciplinary action, while independent recall/blacklist/notify do.
	if remove != nil {
		out = append(out, *remove)
	} else if mute != nil {
		out = append(out, *mute)
	} else if reply != nil {
		out = append(out, *reply)
	}
	if blacklist {
		out = append(out, Action{Type: ActionBlacklist})
	}
	out = append(out, notifies...)
	return out
}

func stronger(current *Action, candidate Action) *Action {
	if current == nil || candidate.Priority > current.Priority ||
		(candidate.Priority == current.Priority && candidate.Duration > current.Duration) {
		copy := candidate
		return &copy
	}
	return current
}

func effectiveThreshold(rule ModerationRule) int {
	if rule.Threshold > 0 {
		return rule.Threshold
	}
	if rule.Count > 0 {
		return rule.Count
	}
	return 1
}

func effectiveCount(rule ModerationRule) int {
	if rule.Count > 0 {
		return rule.Count
	}
	if rule.Threshold > 0 {
		return rule.Threshold
	}
	return 1
}

// ValidateRule rejects malformed runtime configuration before it reaches the
// processing loop.
func ValidateRule(rule ModerationRule) error {
	if rule.Window < 0 {
		return fmt.Errorf("规则 %d（%s）的时间窗口不得为负数", rule.ID, rule.Name)
	}
	if rule.Cooldown < 0 {
		return fmt.Errorf("规则 %d（%s）的冷却时间不得为负数", rule.ID, rule.Name)
	}
	switch rule.Mode {
	case ModeAuto, ModeObserve, ModeDryRun:
	default:
		return fmt.Errorf("规则 %d（%s）的执行模式 %q 未知", rule.ID, rule.Name, rule.Mode)
	}
	switch rule.Matcher {
	case MatcherExact, MatcherContains, MatcherPrefix, MatcherRegex:
		if rule.Pattern == "" {
			return fmt.Errorf("规则 %d（%s）缺少匹配内容", rule.ID, rule.Name)
		}
	case MatcherLength, MatcherLines:
		if rule.Threshold <= 0 && rule.Count <= 0 {
			return fmt.Errorf("规则 %d（%s）缺少阈值", rule.ID, rule.Name)
		}
	case MatcherImageCount, MatcherRenameCount:
		if rule.Count <= 0 && rule.Threshold <= 0 {
			return fmt.Errorf("规则 %d（%s）缺少触发次数", rule.ID, rule.Name)
		}
	case MatcherSemantic:
		threshold := rule.SemanticThreshold
		if threshold == 0 && rule.Threshold > 0 {
			threshold = float64(rule.Threshold) / 100
		}
		if threshold <= 0 || threshold > 1 {
			return fmt.Errorf("规则 %d（%s）的 AI 语义阈值必须大于 0 且不超过 1", rule.ID, rule.Name)
		}
	case MatcherBlacklist:
	default:
		return fmt.Errorf("规则 %d（%s）的匹配类型 %q 未知", rule.ID, rule.Name, rule.Matcher)
	}
	for _, action := range rule.Actions {
		switch action.Type {
		case ActionReply, ActionRecall, ActionMute, ActionRemove, ActionBlacklist, ActionNotify, ActionRename:
		default:
			return fmt.Errorf("规则 %d（%s）的动作 %q 未知", rule.ID, rule.Name, action.Type)
		}
	}
	return nil
}

// WeightedLength applies the ZCG-compatible calculation: every Han character
// weighs two and every other Unicode code point weighs one.
func WeightedLength(text string) int { return weightedLength(text) }

func weightedLength(text string) int {
	weight := 0
	for _, r := range text {
		if unicode.Is(unicode.Han, r) {
			weight += 2
		} else {
			weight++
		}
	}
	return weight
}

// LineCount counts logical lines without treating an empty message as one line.
func LineCount(text string) int { return lineCount(text) }

func lineCount(text string) int {
	if text == "" {
		return 0
	}
	return strings.Count(text, "\n") + 1
}

func withinWindow(now, event time.Time, window time.Duration) bool {
	delta := now.Sub(event)
	return delta >= 0 && delta <= window
}

func subjectKey(message Message) string {
	return fmt.Sprintf("%s/%d/%d", message.AccountID, message.GroupID, message.UserID)
}

func ruleIdentity(rule ModerationRule, index int) string {
	if rule.ID != 0 {
		return fmt.Sprintf("id:%d", rule.ID)
	}
	return fmt.Sprintf("index:%d:%s:%s", index, rule.Name, rule.Pattern)
}

func cooldownKey(rule ModerationRule, index int, message Message) string {
	return fmt.Sprintf("%s/%s", ruleIdentity(rule, index), subjectKey(message))
}

// DefaultRules returns the built-in ZCG-compatible deterministic rule template.
// The caller chooses the group at runtime; groupID=0 means all enabled groups.
func DefaultRules(groupID int64) []ModerationRule {
	exemptRoles := []MemberRole{RoleOwner, RoleAdmin}
	recall := []RuleAction{{Type: ActionRecall}}
	return []ModerationRule{
		{GroupID: groupID, Name: "加权字符超过100", Matcher: MatcherLength, Threshold: 100, Cooldown: time.Minute, Priority: 100, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "加权字符超过200", Matcher: MatcherLength, Threshold: 200, Cooldown: time.Minute, Priority: 200, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "超过4行", Matcher: MatcherLines, Threshold: 4, Cooldown: time.Minute, Priority: 100, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "图片消息", Matcher: MatcherImageCount, Count: 1, Cooldown: time.Minute, Priority: 100, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "图片10分钟3次", Matcher: MatcherImageCount, Count: 3, Window: 10 * time.Minute, Priority: 200, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "恢复群名片", Matcher: MatcherRenameCount, Count: 1, Priority: 100, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "改名累计5次", Matcher: MatcherRenameCount, Count: 5, Priority: 200, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
		{GroupID: groupID, Name: "黑名单成员", Matcher: MatcherBlacklist, Priority: 300, Mode: ModeAuto, Enabled: true, ExemptRoles: exemptRoles, Actions: recall},
	}
}

// DefaultZCGRules is the explicit name used by setup/import code.
func DefaultZCGRules(groupID int64) []ModerationRule { return DefaultRules(groupID) }

// DefaultSemanticRules are suggestions only. An administrator can opt a rule
// into auto mode after reviewing observed confidence and false positives.
func DefaultSemanticRules(groupID int64) []ModerationRule {
	exemptRoles := []MemberRole{RoleOwner, RoleAdmin}
	return []ModerationRule{
		{GroupID: groupID, Name: "AI广告识别", Matcher: MatcherSemantic, Pattern: "advertisement", SemanticThreshold: 0.85, Priority: 50, Mode: ModeObserve, Enabled: true, ExemptRoles: exemptRoles, Actions: []RuleAction{{Type: ActionRecall}}},
		{GroupID: groupID, Name: "AI辱骂识别", Matcher: MatcherSemantic, Pattern: "abuse", SemanticThreshold: 0.85, Priority: 50, Mode: ModeObserve, Enabled: true, ExemptRoles: exemptRoles, Actions: []RuleAction{{Type: ActionRecall}}},
		{GroupID: groupID, Name: "AI诈骗识别", Matcher: MatcherSemantic, Pattern: "scam", SemanticThreshold: 0.90, Priority: 60, Mode: ModeObserve, Enabled: true, ExemptRoles: exemptRoles, Actions: []RuleAction{{Type: ActionRecall}}},
	}
}

// SortRules returns a copy sorted by descending priority, with stable ID/name
// tie-breakers. Engine evaluation itself preserves configured order for ties.
func SortRules(rules []ModerationRule) []ModerationRule {
	out := cloneRules(rules)
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Priority != out[j].Priority {
			return out[i].Priority > out[j].Priority
		}
		if out[i].ID != out[j].ID {
			return out[i].ID < out[j].ID
		}
		return out[i].Name < out[j].Name
	})
	return out
}

func cloneRules(rules []ModerationRule) []ModerationRule {
	out := make([]ModerationRule, len(rules))
	for i, rule := range rules {
		out[i] = rule
		out[i].ExemptRoles = append([]MemberRole(nil), rule.ExemptRoles...)
		out[i].ExemptUserIDs = append([]int64(nil), rule.ExemptUserIDs...)
		out[i].Actions = append([]RuleAction(nil), rule.Actions...)
	}
	return out
}
