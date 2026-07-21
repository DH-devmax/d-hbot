package appcore

import (
	"context"
	"fmt"
	"strings"
	"time"

	"dh/internal/groupmgr"
)

type RestoreMemberResult struct {
	Matched  int
	Restored int
	Failed   int
	Skipped  int
	Errors   []string
}

func (manager *Manager) RestoreOriginalCardNames(ctx context.Context, groupID int64, userIDs []int64) (RestoreMemberResult, error) {
	result := RestoreMemberResult{}
	if err := manager.RequireManager(ctx, groupID); err != nil {
		return result, err
	}
	selected := make(map[int64]bool, len(userIDs))
	for _, userID := range userIDs {
		selected[userID] = true
	}
	members, err := manager.Store.ListMembers(ctx, manager.AccountID, groupID)
	if err != nil {
		return result, err
	}
	selfID := mustAccountID(manager.AccountID)
	for _, member := range members {
		if len(selected) > 0 && !selected[member.UserID] {
			continue
		}
		original := strings.TrimSpace(member.OriginalCardName)
		if !member.Present || excludedCardMember(member, selfID) || inactiveMember(member) || original == "" {
			result.Skipped++
			continue
		}
		result.Matched++
		if member.CardName == original && member.ManagedCardName == "" {
			result.Skipped++
			continue
		}
		renameErr := manager.Gateway.Rename(ctx, groupID, groupmgr.MemberRef{UserID: member.UserID, NIMID: member.NIMID}, original)
		manager.recordManualAction(ctx, groupID, member.UserID, groupmgr.ActionRename, 0, renameErr)
		if renameErr != nil {
			result.Failed++
			result.Errors = append(result.Errors, fmt.Sprintf("%d: %v", member.UserID, renameErr))
			continue
		}
		member.CardName, member.ManagedCardName, member.CardSuffix = original, "", ""
		member.LockedCardName, member.CardStatus, member.UpdatedAt = original, groupmgr.CardUnmanaged, time.Now().UTC()
		if err := manager.Store.UpsertMember(ctx, member); err != nil {
			result.Failed++
			result.Errors = append(result.Errors, fmt.Sprintf("%d: %v", member.UserID, err))
			continue
		}
		result.Restored++
		manager.audit(ctx, groupID, member.UserID, "card_original_restored", "info", original)
		select {
		case <-ctx.Done():
			return result, ctx.Err()
		case <-time.After(250 * time.Millisecond):
		}
	}
	return result, nil
}
