package appcore

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"

	"dh/internal/cardnames"
	"dh/internal/groupmgr"
)

type CardStats struct {
	Settings    groupmgr.GroupCardSettings
	Present     int
	Pending     int
	Running     int
	Verified    int
	Failed      int
	OfflineNew  int
	UnreadNew   int
	CoverageGap int
	RecentJobs  []groupmgr.CardRenameJob
}

func (manager *Manager) PreviewCardNames(ctx context.Context, groupID int64) (groupmgr.CardPreview, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return groupmgr.CardPreview{}, err
	}
	return manager.previewCardNames(ctx, groupID)
}

func (manager *Manager) previewCardNames(ctx context.Context, groupID int64) (groupmgr.CardPreview, error) {
	settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, groupID)
	if err != nil {
		return groupmgr.CardPreview{}, err
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return groupmgr.CardPreview{}, err
	}
	jobs, err := manager.Store.ListCardJobs(ctx, manager.AccountID, groupID, cardnames.Capacity)
	if err != nil {
		return groupmgr.CardPreview{}, err
	}
	latestJob := make(map[int64]groupmgr.CardRenameJob)
	for _, job := range jobs {
		if _, exists := latestJob[job.UserID]; !exists {
			latestJob[job.UserID] = job
		}
	}
	sort.SliceStable(members, func(left, right int) bool {
		leftTime, rightTime := members[left].DiscoveredAt, members[right].DiscoveredAt
		if !leftTime.Equal(rightTime) {
			if leftTime.IsZero() {
				return true
			}
			if rightTime.IsZero() {
				return false
			}
			return leftTime.Before(rightTime)
		}
		return members[left].UserID < members[right].UserID
	})
	selfID, _ := strconv.ParseInt(manager.AccountID, 10, 64)
	usedNames := make(map[string]bool, len(members)*2)
	usedSuffixes := make(map[string]bool)
	for _, member := range members {
		if member.CardSuffix != "" {
			usedSuffixes[member.CardSuffix] = true
		}
		if suffix := cardnames.ExtractSuffix(member.CardName); suffix != "" {
			usedSuffixes[suffix] = true
		}
		if excludedCardRenameMember(member, selfID) {
			if name := strings.TrimSpace(resolvedName(member)); name != "" {
				usedNames[name] = true
			}
		}
		if member.ManagedCardName != "" {
			usedNames[member.ManagedCardName] = true
		}
	}
	for _, job := range latestJob {
		if job.State == groupmgr.RenameQueued || job.State == groupmgr.RenameRunning || job.State == groupmgr.RenamePaused {
			usedNames[job.DesiredName] = true
		}
	}
	preview := groupmgr.CardPreview{GroupID: groupID, Prefix: settings.Prefix}
	nextIndex := 1
	for _, member := range members {
		item := groupmgr.CardPlan{Member: member, OriginalName: firstNonEmpty(member.OriginalCardName, member.CardName, member.Nickname)}
		if excludedCardRenameMember(member, selfID) {
			item.Status, item.Reason = groupmgr.CardExcluded, "群主、管理员、当前账号、系统账号或失效账号"
			preview.Excluded++
			preview.Items = append(preview.Items, item)
			continue
		}
		if member.UserID <= 0 && member.NIMID == "" {
			item.Status, item.Reason = groupmgr.CardConflict, "缺少 userId 和 nimId"
			preview.MissingIdentity++
			preview.Items = append(preview.Items, item)
			continue
		}
		if job, exists := latestJob[member.UserID]; exists && (job.State == groupmgr.RenameQueued || job.State == groupmgr.RenameRunning || job.State == groupmgr.RenamePaused) {
			item.SuggestedName, item.Suffix = job.DesiredName, job.Suffix
			if job.State == groupmgr.RenameRunning {
				item.Status = groupmgr.CardRunning
			} else {
				item.Status = groupmgr.CardQueued
			}
			preview.Items = append(preview.Items, item)
			continue
		}
		if member.ManagedCardName != "" && member.CardName == member.ManagedCardName && member.CardStatus == groupmgr.CardVerified {
			item.SuggestedName, item.Suffix, item.Status = member.ManagedCardName, member.CardSuffix, groupmgr.CardVerified
			preview.AlreadyManaged++
			preview.Items = append(preview.Items, item)
			continue
		}
		if member.ManagedCardName != "" {
			item.SuggestedName, item.Suffix = member.ManagedCardName, member.CardSuffix
		} else {
			item.SuggestedName = cardnames.FirstAvailable(cardnames.Candidates(item.OriginalName), usedNames)
		}
		if item.SuggestedName == "" {
			for nextIndex <= cardnames.Capacity {
				name, suffix, suffixErr := cardnames.NumberedName(settings.Prefix, nextIndex)
				nextIndex++
				if suffixErr != nil {
					return groupmgr.CardPreview{}, suffixErr
				}
				if usedSuffixes[suffix] || usedNames[name] {
					continue
				}
				item.SuggestedName, item.Suffix = name, suffix
				usedSuffixes[suffix] = true
				break
			}
		}
		if item.SuggestedName == "" {
			item.Status, item.Reason = groupmgr.CardConflict, cardnames.ErrCapacity.Error()
			preview.Conflicts++
		} else {
			item.Status = groupmgr.CardPlanned
			usedNames[item.SuggestedName] = true
			preview.WillRename++
		}
		preview.Items = append(preview.Items, item)
	}
	if settings.ReportedCount > settings.ResolvedCount {
		preview.CoverageGap = settings.ReportedCount - settings.ResolvedCount
	}
	return preview, nil
}

func excludedCardMember(member groupmgr.Member, selfID int64) bool {
	return member.UserID == selfID || member.Role == groupmgr.RoleOwner || member.Role == groupmgr.RoleAdmin || member.Role == groupmgr.RoleBot
}

func excludedCardRenameMember(member groupmgr.Member, selfID int64) bool {
	return excludedCardMember(member, selfID) || inactiveMember(member)
}

func (manager *Manager) QueueCardPreview(ctx context.Context, groupID int64, overrides map[int64]string) (int, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return 0, err
	}
	lock := manager.groupLock(groupID)
	lock.Lock()
	defer lock.Unlock()
	preview, err := manager.previewCardNames(ctx, groupID)
	if err != nil {
		return 0, err
	}
	used := make(map[string]int64)
	for _, item := range preview.Items {
		if item.Status == groupmgr.CardExcluded || item.Status == groupmgr.CardVerified {
			used[firstNonEmpty(item.Member.ManagedCardName, item.Member.CardName)] = item.Member.UserID
		}
	}
	now := time.Now().UTC()
	type pendingPlan struct {
		item    groupmgr.CardPlan
		desired string
	}
	pending := make([]pendingPlan, 0, preview.WillRename)
	for _, item := range preview.Items {
		if item.Status != groupmgr.CardPlanned && item.Status != groupmgr.CardFailed {
			continue
		}
		desired := item.SuggestedName
		if override := strings.TrimSpace(overrides[item.Member.UserID]); override != "" {
			if item.Suffix != "" {
				return 0, fmt.Errorf("%s 的固定编号后缀只读", displayName(item.Member))
			}
			if len(cardnames.Normalize(override)) != 2 {
				return 0, fmt.Errorf("%s 的自定义简称需要两个有效字符", displayName(item.Member))
			}
			desired = override
		}
		if owner, exists := used[desired]; exists && owner != item.Member.UserID {
			return 0, fmt.Errorf("名称 %s 与成员 %d 冲突", desired, owner)
		}
		used[desired] = item.Member.UserID
		pending = append(pending, pendingPlan{item: item, desired: desired})
	}
	queued := 0
	for _, plan := range pending {
		item, desired := plan.item, plan.desired
		job := groupmgr.CardRenameJob{
			AccountID: manager.AccountID, GroupID: groupID, UserID: item.Member.UserID, NIMID: item.Member.NIMID,
			OriginalName: item.OriginalName, DesiredName: desired, Suffix: item.Suffix, Source: groupmgr.JoinBaseline,
			State: groupmgr.RenameQueued, CreatedAt: now, UpdatedAt: now,
		}
		if err := manager.Store.EnqueueCardJob(ctx, &job); err != nil {
			return queued, err
		}
		member := item.Member
		member.CardSuffix, member.CardStatus, member.UpdatedAt = item.Suffix, groupmgr.CardQueued, now
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			return queued, err
		}
		queued++
	}
	manager.audit(ctx, groupID, 0, "card_batch_queued", "info", fmt.Sprintf("已排队 %d 人，覆盖缺口 %d", queued, preview.CoverageGap))
	return queued, nil
}

func (manager *Manager) queueAutoCardMember(ctx context.Context, group groupmgr.Group, settings groupmgr.GroupCardSettings, member groupmgr.Member, welcome bool) error {
	if !settings.AutoRename || settings.Paused || excludedCardRenameMember(member, mustAccountID(manager.AccountID)) {
		return nil
	}
	preview, err := manager.previewCardNames(ctx, group.GroupID)
	if err != nil {
		return err
	}
	for _, item := range preview.Items {
		if item.Member.UserID != member.UserID || item.Status != groupmgr.CardPlanned {
			continue
		}
		now := time.Now().UTC()
		job := groupmgr.CardRenameJob{
			AccountID: manager.AccountID, GroupID: group.GroupID, UserID: member.UserID, NIMID: member.NIMID,
			OriginalName: item.OriginalName, DesiredName: item.SuggestedName, Suffix: item.Suffix,
			Source: member.JoinSource, State: groupmgr.RenameQueued, WelcomePending: welcome,
			CreatedAt: now, UpdatedAt: now,
		}
		if err := manager.Store.EnqueueCardJob(ctx, &job); err != nil {
			return err
		}
		member.CardSuffix, member.CardStatus, member.UpdatedAt = item.Suffix, groupmgr.CardQueued, now
		return manager.Store.UpsertMember(ctx, member)
	}
	return nil
}

func mustAccountID(value string) int64 {
	parsed, _ := strconv.ParseInt(value, 10, 64)
	return parsed
}

func (manager *Manager) SaveCardSettings(ctx context.Context, groupID int64, prefix string, autoRename, paused, updateExisting bool) (int, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return 0, err
	}
	prefix = strings.TrimSpace(prefix)
	if prefix == "" {
		prefix = "DH"
	}
	for _, current := range prefix {
		if unicode.IsControl(current) {
			return 0, errors.New("群名片前缀含控制字符")
		}
	}
	settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, groupID)
	if err != nil {
		return 0, err
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return 0, err
	}
	if updateExisting && prefix != settings.Prefix {
		reserved := make(map[string]int64)
		for _, member := range members {
			if member.CardSuffix == "" {
				reserved[resolvedName(member)] = member.UserID
			}
		}
		for _, member := range members {
			if member.CardSuffix == "" {
				continue
			}
			desired := prefix + "群员" + member.CardSuffix
			if owner, exists := reserved[desired]; exists && owner != member.UserID {
				return 0, fmt.Errorf("新前缀与成员 %d 的名称 %s 冲突", owner, desired)
			}
			reserved[desired] = member.UserID
		}
	}
	previousPrefix := settings.Prefix
	settings.Prefix, settings.AutoRename, settings.Paused, settings.UpdatedAt = prefix, autoRename, paused, time.Now().UTC()
	if err := manager.Store.UpsertCardSettings(ctx, settings); err != nil {
		return 0, err
	}
	if !updateExisting || prefix == previousPrefix || len(members) == 0 {
		return 0, nil
	}
	queued := 0
	now := time.Now().UTC()
	for _, member := range members {
		if member.CardSuffix == "" || excludedCardRenameMember(member, mustAccountID(manager.AccountID)) {
			continue
		}
		desired := prefix + "群员" + member.CardSuffix
		if desired == member.ManagedCardName && desired == member.CardName {
			continue
		}
		job := groupmgr.CardRenameJob{AccountID: manager.AccountID, GroupID: groupID, UserID: member.UserID, NIMID: member.NIMID, OriginalName: member.CardName, DesiredName: desired, Suffix: member.CardSuffix, Source: groupmgr.JoinBaseline, State: groupmgr.RenameQueued, CreatedAt: now, UpdatedAt: now}
		if err := manager.Store.EnqueueCardJob(ctx, &job); err != nil {
			return queued, err
		}
		member.CardStatus, member.UpdatedAt = groupmgr.CardQueued, now
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			return queued, err
		}
		queued++
	}
	manager.audit(ctx, groupID, 0, "card_prefix_changed", "info", fmt.Sprintf("前缀改为 %s，排队更新 %d 人", prefix, queued))
	return queued, nil
}

func (manager *Manager) RunCardJobs(ctx context.Context, limit int) (int, error) {
	if limit <= 0 {
		limit = 1
	}
	processed := 0
	for processed < limit {
		job, err := manager.Store.NextCardJob(ctx, manager.AccountID, time.Now().UTC(), !manager.IsPaused())
		if errors.Is(err, sql.ErrNoRows) {
			return processed, nil
		}
		if err != nil {
			return processed, err
		}
		settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, job.GroupID)
		if err != nil {
			return processed, err
		}
		if settings.Paused {
			return processed, nil
		}
		if job.State == groupmgr.RenameVerified && job.WelcomePending {
			if err := manager.sendCardWelcome(ctx, &job); err != nil {
				return processed, err
			}
			processed++
			continue
		}
		if err := manager.runCardJob(ctx, &job); err != nil {
			return processed, err
		}
		processed++
	}
	return processed, nil
}

func (manager *Manager) runCardJob(ctx context.Context, job *groupmgr.CardRenameJob) error {
	if err := manager.RequireManager(ctx, job.GroupID); err != nil {
		if errors.Is(err, ErrManagerRequired) {
			job.State, job.LastError, job.UpdatedAt = groupmgr.RenamePaused, err.Error(), time.Now().UTC()
			if member, loadErr := manager.Store.GetMember(ctx, manager.AccountID, job.GroupID, job.UserID); loadErr == nil {
				member.CardStatus, member.UpdatedAt = groupmgr.CardPlanned, job.UpdatedAt
				_ = manager.Store.UpsertMember(ctx, member)
			}
			manager.audit(ctx, job.GroupID, job.UserID, "card_rename_permission_paused", "warn", err.Error())
			return manager.Store.UpdateCardJob(ctx, *job)
		}
		return manager.deferCardJob(ctx, job, err)
	}
	member, err := manager.Store.GetMember(ctx, manager.AccountID, job.GroupID, job.UserID)
	if err != nil {
		return manager.deferCardJob(ctx, job, err)
	}
	if inactiveMember(member) {
		job.State, job.LastError, job.NextAttemptAt, job.UpdatedAt = groupmgr.RenameCanceled, "失效账号已跳过，请使用清理失效", time.Time{}, time.Now().UTC()
		member.CardStatus, member.UpdatedAt = groupmgr.CardExcluded, job.UpdatedAt
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			return err
		}
		manager.audit(ctx, job.GroupID, job.UserID, "card_rename_inactive_skipped", "info", inactiveMemberDisplay(member))
		return manager.Store.UpdateCardJob(ctx, *job)
	}
	if !member.Present || excludedCardMember(member, mustAccountID(manager.AccountID)) {
		job.State, job.LastError, job.UpdatedAt = groupmgr.RenameFailed, "成员已离群或角色已排除", time.Now().UTC()
		member.CardStatus, member.UpdatedAt = groupmgr.CardFailed, job.UpdatedAt
		_ = manager.Store.UpsertMember(ctx, member)
		return manager.Store.UpdateCardJob(ctx, *job)
	}
	job.State, job.Attempts, job.LastError, job.UpdatedAt = groupmgr.RenameRunning, job.Attempts+1, "", time.Now().UTC()
	if err := manager.Store.UpdateCardJob(ctx, *job); err != nil {
		return err
	}
	member.CardStatus, member.UpdatedAt = groupmgr.CardRunning, job.UpdatedAt
	if err := manager.Store.UpsertMember(ctx, member); err != nil {
		return err
	}
	if err := manager.Gateway.Rename(ctx, job.GroupID, groupmgr.MemberRef{UserID: member.UserID, NIMID: firstNonEmpty(job.NIMID, member.NIMID)}, job.DesiredName); err != nil {
		return manager.deferCardJob(ctx, job, err)
	}
	verified, err := manager.verifyCardRename(ctx, member, job.DesiredName)
	if err != nil {
		return manager.deferCardJob(ctx, job, err)
	}
	if !verified {
		return manager.deferCardJob(ctx, job, errors.New("改名回执成功，但成员快照尚未返回目标名称"))
	}
	job.State, job.LastError, job.NextAttemptAt, job.UpdatedAt = groupmgr.RenameVerified, "", time.Time{}, time.Now().UTC()
	if err := manager.Store.CompleteCardRename(ctx, member, *job); err != nil {
		return err
	}
	manager.recordAction(ctx, groupmgr.ActionRecord{AccountID: manager.AccountID, GroupID: job.GroupID, UserID: job.UserID, Type: groupmgr.ActionRename, Mode: groupmgr.RuleAuto, Reason: "群名片自动编排", Success: true, CreatedAt: time.Now().UTC()})
	manager.audit(ctx, job.GroupID, job.UserID, "card_rename_verified", "info", job.DesiredName)
	if job.WelcomePending {
		return manager.sendCardWelcome(ctx, job)
	}
	return nil
}

func (manager *Manager) verifyCardRename(ctx context.Context, member groupmgr.Member, desiredName string) (bool, error) {
	delays := []time.Duration{250 * time.Millisecond, 500 * time.Millisecond, time.Second}
	var lastErr error
	for _, delay := range delays {
		select {
		case <-ctx.Done():
			return false, ctx.Err()
		case <-time.After(delay):
		}
		roster, err := manager.Gateway.ListMembers(ctx, member.GroupID)
		if err != nil {
			lastErr = err
			continue
		}
		for _, remote := range roster.Members {
			identityMatch := remote.UserID == member.UserID || (member.NIMID != "" && remote.NIMID == member.NIMID)
			if identityMatch && strings.TrimSpace(remote.CardName) == desiredName {
				return true, nil
			}
		}
	}
	return false, lastErr
}

var cardRetryDelays = []time.Duration{5 * time.Second, 30 * time.Second, 2 * time.Minute, 10 * time.Minute, 30 * time.Minute}

func (manager *Manager) deferCardJob(ctx context.Context, job *groupmgr.CardRenameJob, cause error) error {
	job.LastError, job.UpdatedAt = cause.Error(), time.Now().UTC()
	if job.Attempts >= len(cardRetryDelays) {
		job.State, job.NextAttemptAt = groupmgr.RenameFailed, time.Time{}
		if member, err := manager.Store.GetMember(ctx, manager.AccountID, job.GroupID, job.UserID); err == nil {
			member.CardStatus, member.UpdatedAt = groupmgr.CardFailed, job.UpdatedAt
			_ = manager.Store.UpsertMember(ctx, member)
		}
		manager.audit(ctx, job.GroupID, job.UserID, "card_rename_failed", "warn", cause.Error())
	} else {
		job.State = groupmgr.RenameQueued
		delayIndex := job.Attempts - 1
		if delayIndex < 0 {
			delayIndex = 0
		}
		job.NextAttemptAt = job.UpdatedAt.Add(cardRetryDelays[delayIndex])
		if member, err := manager.Store.GetMember(ctx, manager.AccountID, job.GroupID, job.UserID); err == nil {
			member.CardStatus, member.UpdatedAt = groupmgr.CardQueued, job.UpdatedAt
			_ = manager.Store.UpsertMember(ctx, member)
		}
		manager.audit(ctx, job.GroupID, job.UserID, "card_rename_retry", "warn", cause.Error())
	}
	return manager.Store.UpdateCardJob(ctx, *job)
}

func (manager *Manager) sendCardWelcome(ctx context.Context, job *groupmgr.CardRenameJob) error {
	group, err := manager.group(ctx, job.GroupID)
	if err != nil {
		return err
	}
	if strings.TrimSpace(group.WelcomeMessage) == "" {
		job.WelcomePending, job.UpdatedAt = false, time.Now().UTC()
		return manager.Store.UpdateCardJob(ctx, *job)
	}
	welcome := strings.ReplaceAll(group.WelcomeMessage, "@[成员]", "@「"+job.DesiredName+"」")
	welcome = strings.ReplaceAll(welcome, "[成员]", job.DesiredName)
	if _, err := manager.Gateway.SendText(ctx, job.GroupID, welcome); err != nil {
		job.LastError, job.NextAttemptAt, job.UpdatedAt = err.Error(), time.Now().UTC().Add(30*time.Second), time.Now().UTC()
		_ = manager.Store.UpdateCardJob(ctx, *job)
		manager.audit(ctx, job.GroupID, job.UserID, "welcome_error", "warn", err.Error())
		return err
	}
	job.WelcomePending, job.UpdatedAt = false, time.Now().UTC()
	if err := manager.Store.UpdateCardJob(ctx, *job); err != nil {
		return err
	}
	manager.audit(ctx, job.GroupID, job.UserID, "welcome", "info", welcome)
	return nil
}

func (manager *Manager) RetryFailedCardJobs(ctx context.Context, groupID int64) error {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return err
	}
	return manager.Store.RetryFailedCardJobs(ctx, manager.AccountID, groupID)
}

func (manager *Manager) MarkCardNoticesRead(ctx context.Context, groupID int64) error {
	return manager.Store.MarkMemberNoticesRead(ctx, manager.AccountID, groupID)
}

func (manager *Manager) CardStats(ctx context.Context, groupID int64) (CardStats, error) {
	settings, err := manager.Store.GetCardSettings(ctx, manager.AccountID, groupID)
	if err != nil {
		return CardStats{}, err
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return CardStats{}, err
	}
	jobs, err := manager.Store.ListCardJobs(ctx, manager.AccountID, groupID, 100)
	if err != nil {
		return CardStats{}, err
	}
	stats := CardStats{Settings: settings, Present: len(members), RecentJobs: jobs}
	if settings.ReportedCount > settings.ResolvedCount {
		stats.CoverageGap = settings.ReportedCount - settings.ResolvedCount
	}
	for _, member := range members {
		switch member.CardStatus {
		case groupmgr.CardQueued, groupmgr.CardPlanned:
			stats.Pending++
		case groupmgr.CardRunning:
			stats.Running++
		case groupmgr.CardVerified:
			stats.Verified++
		case groupmgr.CardFailed:
			stats.Failed++
		}
		if member.JoinSource == groupmgr.JoinOffline {
			stats.OfflineNew++
		}
		if !member.NoticeRead && member.JoinSource != groupmgr.JoinBaseline {
			stats.UnreadNew++
		}
	}
	return stats, nil
}
