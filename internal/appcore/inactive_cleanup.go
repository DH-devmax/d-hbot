package appcore

import (
	"context"
	"fmt"
	"strings"
	"time"

	"dh/internal/groupmgr"
)

type InactiveCleanupResult struct {
	Matched int
	Removed int
	Failed  int
	Skipped int
	Errors  []string
}

func (manager *Manager) InactiveMembers(ctx context.Context, groupID int64) ([]groupmgr.Member, error) {
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return nil, err
	}
	if err := manager.SyncMembers(ctx, groupID); err != nil {
		return nil, err
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return nil, err
	}
	selfID := mustAccountID(manager.AccountID)
	candidates := make([]groupmgr.Member, 0)
	for _, member := range members {
		if member.Present && !excludedCardMember(member, selfID) && inactiveMember(member) {
			candidates = append(candidates, member)
		}
	}
	return candidates, nil
}

func (manager *Manager) CleanupInactiveMembers(ctx context.Context, groupID int64, userIDs []int64) (InactiveCleanupResult, error) {
	result := InactiveCleanupResult{}
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return result, err
	}
	requested := make(map[int64]bool, len(userIDs))
	for _, userID := range userIDs {
		requested[userID] = true
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return result, err
	}
	selfID := mustAccountID(manager.AccountID)
	for _, member := range members {
		if !requested[member.UserID] {
			continue
		}
		if !member.Present || excludedCardMember(member, selfID) || !inactiveMember(member) {
			result.Skipped++
			continue
		}
		result.Matched++
		removeErr := manager.Gateway.RemoveMember(ctx, groupID, member.UserID)
		manager.recordManualAction(ctx, groupID, member.UserID, groupmgr.ActionRemove, 0, removeErr)
		if removeErr != nil {
			result.Failed++
			result.Errors = append(result.Errors, fmt.Sprintf("%d: %v", member.UserID, removeErr))
			manager.audit(ctx, groupID, member.UserID, "inactive_member_cleanup_failed", "warn", removeErr.Error())
			continue
		}
		member.Present, member.UpdatedAt = false, time.Now().UTC()
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			result.Failed++
			result.Errors = append(result.Errors, fmt.Sprintf("%d: %v", member.UserID, err))
			continue
		}
		result.Removed++
		manager.audit(ctx, groupID, member.UserID, "inactive_member_cleanup_removed", "info", inactiveMemberDisplay(member))
	}
	return result, nil
}

func inactiveMember(member groupmgr.Member) bool {
	_, inactive := InactiveMemberLabel(member)
	return inactive
}

func inactiveMemberDisplay(member groupmgr.Member) string {
	label, _ := InactiveMemberLabel(member)
	return label
}

func InactiveMemberLabel(member groupmgr.Member) (string, bool) {
	if inactiveAccountState(member.AccountState) {
		return inactiveAccountStateLabel(member.AccountState), true
	}
	if inactiveAccountName(member.CardName) {
		return strings.TrimSpace(member.CardName), true
	}
	if inactiveAccountName(member.Nickname) {
		return strings.TrimSpace(member.Nickname), true
	}
	return "", false
}

func inactiveAccountState(value string) bool {
	switch strings.ToUpper(strings.TrimSpace(value)) {
	case "ACCOUNT_STATE_BAN", "ACCOUNT_STATE_INACTIVATED", "ACCOUNT_STATUS_CANCELLED":
		return true
	default:
		return false
	}
}

func inactiveAccountStateLabel(value string) string {
	switch strings.ToUpper(strings.TrimSpace(value)) {
	case "ACCOUNT_STATUS_CANCELLED", "ACCOUNT_STATE_INACTIVATED":
		return "该用户已注销"
	case "ACCOUNT_STATE_BAN":
		return "已封禁用户"
	default:
		return "失效账号"
	}
}

func inactiveAccountName(value string) bool {
	switch strings.TrimSpace(value) {
	case "该用户已注销", "已封禁用户":
		return true
	default:
		return false
	}
}
