package appcore

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"dh/internal/groupmgr"
	"dh/internal/store"
)

func newCardTestManager(t *testing.T, members []groupmgr.Member) (*Manager, *fakeGateway, *store.Store) {
	t.Helper()
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}, members: members}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	return manager, gateway, database
}

func TestCardPreviewAbbreviatesResolvesConflictsAndNumbersMissingNames(t *testing.T) {
	manager, _, _ := newCardTestManager(t, []groupmgr.Member{
		{GroupID: 7, UserID: 8, CardName: "广州校长", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 9, CardName: "广州校长", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 10, CardName: ".", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 11, CardName: "管理员", Role: groupmgr.RoleAdmin},
	})
	preview, err := manager.PreviewCardNames(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	got := make(map[int64]groupmgr.CardPlan)
	for _, item := range preview.Items {
		got[item.Member.UserID] = item
	}
	if got[8].SuggestedName != "广校" {
		t.Fatalf("first abbreviation=%q", got[8].SuggestedName)
	}
	if got[9].SuggestedName == "" || got[9].SuggestedName == "广校" {
		t.Fatalf("collision fallback=%q", got[9].SuggestedName)
	}
	if got[10].SuggestedName != "DH群员0001" || got[10].Suffix != "0001" {
		t.Fatalf("numbered=%+v", got[10])
	}
	if got[11].Status != groupmgr.CardExcluded || got[9001].Status != groupmgr.CardExcluded {
		t.Fatalf("role exclusions admin=%s self=%s", got[11].Status, got[9001].Status)
	}
}

func TestOnlineJoinRenamesBeforeWelcomeAndSwitchesLockBaseline(t *testing.T) {
	manager, gateway, database := newCardTestManager(t, nil)
	group, err := database.GetGroup(context.Background(), "9001", 7)
	if err != nil {
		t.Fatal(err)
	}
	group.WelcomeMessage = "欢迎 @[成员]"
	if err := database.UpsertGroup(context.Background(), group); err != nil {
		t.Fatal(err)
	}
	if _, err := manager.SaveCardSettings(context.Background(), 7, "DH", true, false, false); err != nil {
		t.Fatal(err)
	}
	gateway.members = append(gateway.members, groupmgr.Member{GroupID: 7, UserID: 8, NIMID: "nim-8", CardName: "广州校长", Role: groupmgr.RoleMember})
	if err := manager.HandleMemberJoined(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	jobs, err := database.ListCardJobs(context.Background(), "9001", 7, 10)
	if err != nil || len(jobs) != 1 || !jobs[0].WelcomePending || jobs[0].DesiredName != "广校" {
		t.Fatalf("jobs=%+v err=%v", jobs, err)
	}
	if len(gateway.texts) != 0 {
		t.Fatalf("welcome sent before rename: %v", gateway.texts)
	}
	if processed, err := manager.RunCardJobs(context.Background(), 1); err != nil || processed != 1 {
		t.Fatalf("processed=%d err=%v", processed, err)
	}
	member, err := database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil {
		t.Fatal(err)
	}
	if member.ManagedCardName != "广校" || member.LockedCardName != "广校" || member.CardStatus != groupmgr.CardVerified {
		t.Fatalf("member=%+v", member)
	}
	if len(gateway.texts) != 1 || gateway.texts[0] != "欢迎 @「广校」" {
		t.Fatalf("welcomes=%v", gateway.texts)
	}
}

func TestOfflineJoinQueuesAutomaticallyAndKeepsUnreadNotice(t *testing.T) {
	manager, gateway, _ := newCardTestManager(t, nil)
	if _, err := manager.SaveCardSettings(context.Background(), 7, "DH", true, false, false); err != nil {
		t.Fatal(err)
	}
	gateway.members = append(gateway.members, groupmgr.Member{GroupID: 7, UserID: 8, CardName: ".", Role: groupmgr.RoleMember})
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	stats, err := manager.CardStats(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	if stats.OfflineNew != 1 || stats.UnreadNew != 1 || stats.Pending != 1 {
		t.Fatalf("stats=%+v", stats)
	}
	if err := manager.MarkCardNoticesRead(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	stats, _ = manager.CardStats(context.Background(), 7)
	if stats.UnreadNew != 0 {
		t.Fatalf("notice still unread: %+v", stats)
	}
}

func TestPrefixChangeCanUpdateOrKeepExistingNumberedMembers(t *testing.T) {
	manager, gateway, database := newCardTestManager(t, []groupmgr.Member{{GroupID: 7, UserID: 8, CardName: ".", Role: groupmgr.RoleMember}})
	if queued, err := manager.QueueCardPreview(context.Background(), 7, nil); err != nil || queued != 1 {
		t.Fatalf("queue=%d err=%v", queued, err)
	}
	if _, err := manager.RunCardJobs(context.Background(), 1); err != nil {
		t.Fatal(err)
	}
	if _, err := manager.SaveCardSettings(context.Background(), 7, "海", true, false, false); err != nil {
		t.Fatal(err)
	}
	member, _ := database.GetMember(context.Background(), "9001", 7, 8)
	if member.ManagedCardName != "DH群员0001" {
		t.Fatalf("keep changed existing=%q", member.ManagedCardName)
	}
	if _, err := manager.SaveCardSettings(context.Background(), 7, "新", true, false, true); err != nil {
		t.Fatal(err)
	}
	jobs, _ := database.ListCardJobs(context.Background(), "9001", 7, 10)
	if jobs[0].DesiredName != "新群员0001" {
		t.Fatalf("prefix update job=%+v members=%+v", jobs[0], gateway.members)
	}
}

func TestRenameFailureIsPersistedForRetry(t *testing.T) {
	manager, gateway, database := newCardTestManager(t, []groupmgr.Member{{GroupID: 7, UserID: 8, CardName: "广州校长", Role: groupmgr.RoleMember}})
	if _, err := manager.QueueCardPreview(context.Background(), 7, nil); err != nil {
		t.Fatal(err)
	}
	gateway.renameErr = errors.New("temporary")
	if processed, err := manager.RunCardJobs(context.Background(), 1); err != nil || processed != 1 {
		t.Fatalf("processed=%d err=%v", processed, err)
	}
	jobs, _ := database.ListCardJobs(context.Background(), "9001", 7, 10)
	if jobs[0].State != groupmgr.RenameQueued || jobs[0].Attempts != 1 || !strings.Contains(jobs[0].LastError, "temporary") {
		t.Fatalf("job=%+v", jobs[0])
	}
	delay := jobs[0].NextAttemptAt.Sub(jobs[0].UpdatedAt)
	if delay < 4*time.Second || delay > 6*time.Second {
		t.Fatalf("retry delay=%s", delay)
	}
}

func TestManualBatchRunsWhileGlobalAutomationIsPaused(t *testing.T) {
	manager, _, database := newCardTestManager(t, []groupmgr.Member{{GroupID: 7, UserID: 8, CardName: "广州校长", Role: groupmgr.RoleMember}})
	manager.SetPaused(true)
	if queued, err := manager.QueueCardPreview(context.Background(), 7, nil); err != nil || queued != 1 {
		t.Fatalf("queued=%d err=%v", queued, err)
	}
	if processed, err := manager.RunCardJobs(context.Background(), 1); err != nil || processed != 1 {
		t.Fatalf("processed=%d err=%v", processed, err)
	}
	member, err := database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil || member.CardStatus != groupmgr.CardVerified {
		t.Fatalf("member=%+v err=%v", member, err)
	}
}

func TestInactiveMemberCleanupExcludesManagersAndActiveNames(t *testing.T) {
	manager, gateway, database := newCardTestManager(t, []groupmgr.Member{
		{GroupID: 7, UserID: 8, CardName: "该用户已注销", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 9, Nickname: "仍有旧昵称", CardName: "仍有旧名片", AccountState: "ACCOUNT_STATUS_CANCELLED", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 10, CardName: "已封禁用户", Role: groupmgr.RoleAdmin},
		{GroupID: 7, UserID: 11, CardName: "普通成员", Role: groupmgr.RoleMember},
	})
	candidates, err := manager.InactiveMembers(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	if len(candidates) != 2 || candidates[0].UserID != 8 || candidates[1].UserID != 9 {
		t.Fatalf("candidates=%+v", candidates)
	}
	preview, err := manager.PreviewCardNames(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	for _, item := range preview.Items {
		if (item.Member.UserID == 8 || item.Member.UserID == 9) && item.Status != groupmgr.CardExcluded {
			t.Fatalf("inactive preview item=%+v", item)
		}
	}
	result, err := manager.CleanupInactiveMembers(context.Background(), 7, []int64{8, 9, 10, 11})
	if err != nil {
		t.Fatal(err)
	}
	if result.Matched != 2 || result.Removed != 2 || result.Failed != 0 || result.Skipped != 2 {
		t.Fatalf("result=%+v", result)
	}
	if len(gateway.removed) != 2 || gateway.removed[0] != 8 || gateway.removed[1] != 9 {
		t.Fatalf("removed=%v", gateway.removed)
	}
	for _, userID := range []int64{8, 9} {
		member, loadErr := database.GetMember(context.Background(), "9001", 7, userID)
		if loadErr != nil || member.Present {
			t.Fatalf("member %d=%+v err=%v", userID, member, loadErr)
		}
	}
}

func TestRestoreOriginalCardNamesSupportsSelectionAndClearsManagedState(t *testing.T) {
	manager, gateway, database := newCardTestManager(t, []groupmgr.Member{
		{GroupID: 7, UserID: 8, CardName: "新名", Role: groupmgr.RoleMember},
		{GroupID: 7, UserID: 9, CardName: "保留", Role: groupmgr.RoleMember},
	})
	member, err := database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil {
		t.Fatal(err)
	}
	member.OriginalCardName, member.ManagedCardName, member.CardSuffix = "原名", "新名", "0001"
	member.LockedCardName, member.CardStatus = "新名", groupmgr.CardVerified
	if err := database.UpsertMember(context.Background(), member); err != nil {
		t.Fatal(err)
	}
	result, err := manager.RestoreOriginalCardNames(context.Background(), 7, []int64{8})
	if err != nil || result.Restored != 1 || result.Failed != 0 {
		t.Fatalf("result=%+v err=%v", result, err)
	}
	member, err = database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil || member.CardName != "原名" || member.ManagedCardName != "" || member.CardSuffix != "" || member.LockedCardName != "原名" || member.CardStatus != groupmgr.CardUnmanaged {
		t.Fatalf("member=%+v err=%v", member, err)
	}
	if len(gateway.actions) == 0 || gateway.actions[len(gateway.actions)-1] != "rename" {
		t.Fatalf("actions=%v", gateway.actions)
	}
}
