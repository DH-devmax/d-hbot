package appcore

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"dh/internal/aiprovider"
	"dh/internal/groupmgr"
	"dh/internal/moderation"
	"dh/internal/prediction"
	"dh/internal/store"
)

type fakeGateway struct {
	groups    []groupmgr.Group
	members   []groupmgr.Member
	actions   []string
	removed   []int64
	texts     []string
	renameErr error
}

func TestDefaultRulesMigrateToGlobalRecallOnly(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	ctx := context.Background()
	legacy := append(moderation.DefaultZCGRules(7), moderation.DefaultSemanticRules(7)...)
	for _, rule := range legacy {
		model := ruleToModel(rule)
		model.GroupID = 7
		model.Actions = []groupmgr.RuleAction{{Type: groupmgr.ActionMute, Duration: 10 * time.Minute}}
		if err := database.UpsertRule(ctx, &model); err != nil {
			t.Fatal(err)
		}
	}
	custom := groupmgr.ModerationRule{GroupID: 7, Name: "自定义规则", Matcher: groupmgr.MatcherContains, Pattern: "x", Mode: groupmgr.RuleAuto, Enabled: true, Actions: []groupmgr.RuleAction{{Type: groupmgr.ActionMute}}, CreatedAt: time.Now(), UpdatedAt: time.Now()}
	if err := database.UpsertRule(ctx, &custom); err != nil {
		t.Fatal(err)
	}
	manager := New(database, &fakeGateway{}, "9001")
	if err := manager.ensureDefaultRules(ctx, 7); err != nil {
		t.Fatal(err)
	}
	global, err := database.ListRules(ctx, 0)
	if err != nil || len(global) != 11 {
		t.Fatalf("global rules=%d err=%v", len(global), err)
	}
	for _, rule := range global {
		if rule.GroupID != 0 || len(rule.Actions) != 1 || rule.Actions[0].Type != groupmgr.ActionRecall {
			t.Fatalf("global rule not recall-only: %+v", rule)
		}
	}
	var legacyCount, customCount int
	if err := database.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM rules WHERE group_id<>0 AND name<>'自定义规则'`).Scan(&legacyCount); err != nil {
		t.Fatal(err)
	}
	if err := database.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM rules WHERE id=?`, custom.ID).Scan(&customCount); err != nil {
		t.Fatal(err)
	}
	if legacyCount != 0 || customCount != 1 {
		t.Fatalf("legacy=%d custom=%d", legacyCount, customCount)
	}
}

func (gateway *fakeGateway) ListGroups(context.Context) ([]groupmgr.Group, error) {
	return gateway.groups, nil
}
func (gateway *fakeGateway) ListMembers(_ context.Context, groupID int64) (groupmgr.MemberRoster, error) {
	members := append([]groupmgr.Member(nil), gateway.members...)
	for index := range members {
		if members[index].GroupID == 0 {
			members[index].GroupID = groupID
		}
	}
	for _, member := range members {
		if member.UserID == 9001 {
			return groupmgr.MemberRoster{Members: members, ReportedCount: len(members), ResolvedCount: len(members), Complete: true, Sources: []string{"fake"}}, nil
		}
	}
	members = append(members, groupmgr.Member{GroupID: groupID, UserID: 9001, CardName: "机器人账号", Role: groupmgr.RoleAdmin})
	return groupmgr.MemberRoster{Members: members, ReportedCount: len(members), ResolvedCount: len(members), Complete: true, Sources: []string{"fake"}}, nil
}
func (gateway *fakeGateway) SendText(_ context.Context, _ int64, text string) (string, error) {
	gateway.texts = append(gateway.texts, text)
	return "sent", nil
}
func (gateway *fakeGateway) Recall(context.Context, int64, int64, string) error {
	gateway.actions = append(gateway.actions, "recall")
	return nil
}
func (gateway *fakeGateway) Mute(context.Context, int64, int64, time.Duration) error {
	gateway.actions = append(gateway.actions, "mute")
	return nil
}
func (gateway *fakeGateway) Unmute(context.Context, int64, int64) error { return nil }
func (gateway *fakeGateway) Rename(_ context.Context, _ int64, ref groupmgr.MemberRef, name string) error {
	gateway.actions = append(gateway.actions, "rename")
	if gateway.renameErr != nil {
		return gateway.renameErr
	}
	for index := range gateway.members {
		if gateway.members[index].UserID == ref.UserID || (ref.NIMID != "" && gateway.members[index].NIMID == ref.NIMID) {
			gateway.members[index].CardName = name
		}
	}
	return nil
}
func (gateway *fakeGateway) RemoveMember(_ context.Context, _ int64, userID int64) error {
	gateway.actions = append(gateway.actions, "remove")
	gateway.removed = append(gateway.removed, userID)
	return nil
}
func (gateway *fakeGateway) SetGroupMute(_ context.Context, _ int64, muted bool) error {
	gateway.actions = append(gateway.actions, fmt.Sprintf("group-mute:%t", muted))
	return nil
}

type fakeProvider struct{}

func (fakeProvider) Decide(context.Context, aiprovider.Request) (aiprovider.Decision, error) {
	return aiprovider.Decision{Reply: "AI回复", Tasks: []aiprovider.Task{{Title: "跟进"}}, Confidence: 0.9}, nil
}

type decisionProvider struct{ decision aiprovider.Decision }

func (provider decisionProvider) Decide(context.Context, aiprovider.Request) (aiprovider.Decision, error) {
	return provider.decision, nil
}

type predictionFixtureSource struct{}

func (predictionFixtureSource) ListGames(context.Context) ([]prediction.Game, error) {
	return []prediction.Game{{ID: "fixture", Name: "测试彩种"}}, nil
}
func (predictionFixtureSource) FetchLive(context.Context, prediction.Game) (prediction.Snapshot, error) {
	return prediction.Snapshot{Game: prediction.Game{ID: "fixture", Name: "测试彩种"}, Period: "20260720001", Result: []int{1, 2, 3}, History: [][]int{{1, 2, 3}, {1, 4, 5}}, UpdatedAt: time.Now(), DataStatus: "已整理"}, nil
}
func (predictionFixtureSource) FetchHistory(context.Context, prediction.Game, int) ([][]int, error) {
	return nil, nil
}

func TestManagerPersistsBeforeActionsAndDeduplicates(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	long := ""
	for index := 0; index < 101; index++ {
		long += "中"
	}
	incoming := Incoming{Sequence: 1, ServerMessageID: "m1", GroupID: 7, UserID: 8, SenderName: "用户", Kind: groupmgr.MessageText, Text: long, SentAt: time.Now()}
	result, err := manager.Process(context.Background(), incoming)
	if err != nil {
		t.Fatal(err)
	}
	if !result.Stored || len(gateway.actions) == 0 {
		t.Fatalf("result=%+v actions=%v", result, gateway.actions)
	}
	duplicate, err := manager.Process(context.Background(), incoming)
	if err != nil || duplicate.Stored {
		t.Fatalf("duplicate=%+v err=%v", duplicate, err)
	}
}

func TestManagerAIReplyAndTask(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	manager.SetProvider(fakeProvider{})
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	result, err := manager.Process(context.Background(), Incoming{Sequence: 2, ServerMessageID: "m2", GroupID: 7, UserID: 8, SenderName: "用户", Kind: groupmgr.MessageText, Text: "@DH 在吗？", SentAt: time.Now()})
	if err != nil {
		t.Fatal(err)
	}
	if !result.AIReplied || result.CreatedTasks != 1 {
		t.Fatalf("result=%+v", result)
	}
	if len(gateway.texts) == 0 || gateway.texts[len(gateway.texts)-1] != "AI回复" {
		t.Fatalf("texts=%v", gateway.texts)
	}
}

func TestPredictionFallsBackToLocalStatisticsWithoutProvider(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	manager.Prediction = prediction.NewWithSource(predictionFixtureSource{})
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	result, err := manager.Process(context.Background(), Incoming{Sequence: 9, ServerMessageID: "prediction", GroupID: 7, UserID: 8, SenderName: "用户", Kind: groupmgr.MessageText, Text: "@DH 预测 测试彩种", SentAt: time.Now()})
	if err != nil {
		t.Fatal(err)
	}
	if !result.AIReplied || len(gateway.texts) != 1 || !strings.Contains(gateway.texts[0], "候选方向") {
		t.Fatalf("result=%+v texts=%v", result, gateway.texts)
	}
}

func TestManagerSemanticRuleClassifiesWithoutReplying(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	manager.SetProvider(decisionProvider{decision: aiprovider.Decision{
		Reply: "这条普通消息不应触发回复", Confidence: .96,
		Actions: []aiprovider.Action{{Type: "ignore", Category: "scam", GroupID: 7, Confidence: .96}},
	}})
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	rules, err := database.ListRules(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	for index := range rules {
		if rules[index].Matcher != groupmgr.MatcherSemantic || rules[index].Pattern != "scam" {
			continue
		}
		rules[index].Mode = groupmgr.RuleAuto
		rules[index].Actions = []groupmgr.RuleAction{
			{Type: groupmgr.ActionRecall},
			{Type: groupmgr.ActionMute, Duration: 10 * time.Minute},
		}
		if err := database.UpsertRule(context.Background(), &rules[index]); err != nil {
			t.Fatal(err)
		}
	}
	result, err := manager.Process(context.Background(), Incoming{
		Sequence: 3, ServerMessageID: "m3", GroupID: 7, UserID: 8, SenderName: "用户",
		Kind: groupmgr.MessageText, Text: "普通陈述", SentAt: time.Now(),
	})
	if err != nil {
		t.Fatal(err)
	}
	if result.AIReplied || len(gateway.texts) != 0 {
		t.Fatalf("ordinary message replied: result=%+v texts=%v", result, gateway.texts)
	}
	if len(gateway.actions) != 2 || gateway.actions[0] != "recall" || gateway.actions[1] != "mute" {
		t.Fatalf("semantic actions=%v", gateway.actions)
	}
}

func TestManagerPauseAndScheduledTaskReminder(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	due := now.Add(-time.Minute)
	if err := database.UpsertMember(context.Background(), groupmgr.Member{
		AccountID: "9001", GroupID: 7, UserID: 8, CardName: "成员甲", Role: groupmgr.RoleMember, UpdatedAt: now,
	}); err != nil {
		t.Fatal(err)
	}
	task := groupmgr.Task{GroupID: 7, AssigneeID: 8, Title: "提交结果", Status: groupmgr.TaskPending, DueAt: &due, CreatedAt: now, UpdatedAt: now}
	if err := database.UpsertTask(context.Background(), &task); err != nil {
		t.Fatal(err)
	}
	if err := manager.RunScheduled(context.Background(), now); err != nil {
		t.Fatal(err)
	}
	if len(gateway.texts) != 1 || gateway.texts[0] != "任务提醒 @成员甲：提交结果" {
		t.Fatalf("texts=%v", gateway.texts)
	}
	if err := manager.RunScheduled(context.Background(), now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if len(gateway.texts) != 1 {
		t.Fatalf("reminder repeated: %v", gateway.texts)
	}

	manager.SetPaused(true)
	long := Incoming{Sequence: 9, ServerMessageID: "paused", GroupID: 7, UserID: 8, Kind: groupmgr.MessageText, Text: string(make([]byte, 220)), SentAt: now}
	result, err := manager.Process(context.Background(), long)
	if err != nil || !result.Stored || result.Actions != 0 {
		t.Fatalf("paused result=%+v err=%v", result, err)
	}
}

func TestScheduledSummaryRunsOncePerDay(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	manager.SetProvider(fakeProvider{})
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	group, err := database.GetGroup(context.Background(), "9001", 7)
	if err != nil {
		t.Fatal(err)
	}
	group.SummarySchedule = "00:00"
	if err := database.UpsertGroup(context.Background(), group); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	message := groupmgr.Message{AccountID: "9001", GroupID: 7, ServerMessageID: "summary-source", UserID: 8, SenderName: "成员", Kind: groupmgr.MessageText, Text: "今天完成了文档", SentAt: now, ReceivedAt: now}
	if _, err := database.InsertMessage(context.Background(), &message); err != nil {
		t.Fatal(err)
	}
	if err := manager.RunScheduled(context.Background(), now); err != nil {
		t.Fatal(err)
	}
	if err := manager.RunScheduled(context.Background(), now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if len(gateway.texts) != 0 {
		t.Fatalf("private summary was sent to group: %v", gateway.texts)
	}
	summaries, err := database.ListDailySummaries(context.Background(), "9001", 10)
	if err != nil || len(summaries) != 1 || summaries[0].Content != "AI回复" || summaries[0].Source != "scheduled" {
		t.Fatalf("summaries=%v err=%v", summaries, err)
	}
}

func TestScheduledGroupMuteUsesLatestDailyTransition(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	now := time.Date(2026, 7, 20, 23, 0, 0, 0, time.Local)
	schedule := &groupmgr.GroupSchedule{AccountID: "9001", Name: "夜间计划", Enabled: true, OpenTime: "08:00", CloseTime: "22:00", Timezone: time.Local.String(), GroupIDs: []int64{7}, UpdatedAt: now}
	if err := database.UpsertGroupSchedule(context.Background(), schedule); err != nil {
		t.Fatal(err)
	}
	if err := database.BindScheduleGroups(context.Background(), "9001", schedule.ID, []int64{7}); err != nil {
		t.Fatal(err)
	}
	if err := manager.RunScheduled(context.Background(), now); err != nil {
		t.Fatal(err)
	}
	if len(gateway.actions) == 0 || gateway.actions[len(gateway.actions)-1] != "group-mute:true" {
		t.Fatalf("actions=%v", gateway.actions)
	}
	if err := manager.RunScheduled(context.Background(), now.Add(time.Minute)); err != nil {
		t.Fatal(err)
	}
	if count := len(gateway.actions); count != 1 {
		t.Fatalf("schedule repeated: %v", gateway.actions)
	}
}

func TestManagerSummaryWithoutMessagesUsesChineseError(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}}}
	manager := New(database, gateway, "9001")
	manager.SetProvider(fakeProvider{})
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if _, err := manager.Summarize(context.Background(), 7); err == nil || err.Error() != "当前群暂无可供摘要的消息" {
		t.Fatalf("summary error=%v", err)
	}
}

func TestBlacklistedMemberSnapshotIsRemoved(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups:  []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{{GroupID: 7, UserID: 8, CardName: "成员", Role: groupmgr.RoleMember}},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	if err := database.UpsertMember(context.Background(), groupmgr.Member{AccountID: "9001", GroupID: 7, UserID: 8, CardName: "成员", Role: groupmgr.RoleMember, Blacklisted: true, UpdatedAt: time.Now()}); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	if len(gateway.actions) != 1 || gateway.actions[0] != "remove" {
		t.Fatalf("actions=%v", gateway.actions)
	}
}

func TestMemberSnapshotBaselinesBeforeWelcomingNewMembers(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{
			{GroupID: 7, UserID: 8, CardName: "成员甲", Role: groupmgr.RoleMember},
			{GroupID: 7, UserID: 9, CardName: "成员乙", Role: groupmgr.RoleMember},
		},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	group, err := database.GetGroup(context.Background(), "9001", 7)
	if err != nil {
		t.Fatal(err)
	}
	group.WelcomeMessage = "欢迎 [成员]"
	if err := database.UpsertGroup(context.Background(), group); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	if len(gateway.texts) != 0 {
		t.Fatalf("initial snapshot sent welcomes: %v", gateway.texts)
	}
	gateway.members = append(gateway.members, groupmgr.Member{GroupID: 7, UserID: 10, CardName: "成员丙", Role: groupmgr.RoleMember})
	if err := manager.HandleMemberJoined(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	if len(gateway.texts) != 1 || gateway.texts[0] != "欢迎 成员丙" {
		t.Fatalf("new member welcomes=%v", gateway.texts)
	}
}

func TestNicknameRuleOnlyRecallsAndKeepsOriginalBaseline(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups:  []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{{GroupID: 7, UserID: 8, Nickname: "账号名称", CardName: "原群名片", Role: groupmgr.RoleMember}},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	gateway.members[0].CardName = "改后名称"
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	if len(gateway.actions) != 1 || gateway.actions[0] != "recall" {
		t.Fatalf("actions=%v", gateway.actions)
	}
	if len(gateway.texts) != 0 {
		t.Fatalf("nickname recall sent notices=%v", gateway.texts)
	}
	member, err := database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil {
		t.Fatal(err)
	}
	if member.LockedCardName != "原群名片" || member.RenameViolations != 1 {
		t.Fatalf("member=%+v", member)
	}
}

func TestResolvedNameUpgradesAnOlderIDOnlySnapshot(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups:  []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{{GroupID: 7, UserID: 8, Nickname: "账号名称", CardName: "群内名称", Role: groupmgr.RoleMember}},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := database.UpsertMember(context.Background(), groupmgr.Member{
		AccountID: "9001", GroupID: 7, UserID: 8, Role: groupmgr.RoleMember, UpdatedAt: time.Now(),
	}); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	member, err := database.GetMember(context.Background(), "9001", 7, 8)
	if err != nil {
		t.Fatal(err)
	}
	if member.CardName != "群内名称" || member.LockedCardName != "群内名称" || member.RenameViolations != 0 {
		t.Fatalf("member=%+v", member)
	}
}

func TestMemberSnapshotRemovesDepartedMembers(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups: []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{
			{GroupID: 7, UserID: 8, CardName: "成员甲", Role: groupmgr.RoleMember},
			{GroupID: 7, UserID: 9, CardName: "成员乙", Role: groupmgr.RoleMember},
		},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	gateway.members = gateway.members[:1]
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	members, err := database.ListMembers(context.Background(), "9001", 7)
	if err != nil {
		t.Fatal(err)
	}
	if len(members) != 2 {
		t.Fatalf("members=%v, want member 8 plus current admin", members)
	}
	for _, member := range members {
		if member.UserID == 9 {
			t.Fatalf("departed member remained: %+v", member)
		}
	}
}

func TestManagerPermissionIsRequiredToEnableGroup(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{
		groups:  []groupmgr.Group{{GroupID: 7, Name: "测试群"}},
		members: []groupmgr.Member{{GroupID: 7, UserID: 9001, CardName: "机器人账号", Role: groupmgr.RoleMember}},
	}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := manager.SyncMembers(context.Background(), 7); err != nil {
		t.Fatal(err)
	}
	if err := manager.SetGroupEnabled(context.Background(), 7, true); !errors.Is(err, ErrManagerRequired) || err.Error() != "需要将账号权限设置为管理" {
		t.Fatalf("enable error=%v", err)
	}
}

func TestManagerBurst1000MessagesIsIdempotent(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "停用群"}}}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}

	var wait sync.WaitGroup
	errors := make(chan error, 1000)
	for index := 0; index < 1000; index++ {
		wait.Add(1)
		go func(sequence int) {
			defer wait.Done()
			_, processErr := manager.Process(context.Background(), Incoming{
				Sequence: uint64(sequence), ServerMessageID: fmt.Sprintf("burst-%04d", sequence),
				GroupID: 7, UserID: int64(sequence%25 + 1), Kind: groupmgr.MessageText,
				Text: "突发消息", SentAt: time.Now(),
			})
			if processErr != nil {
				errors <- processErr
			}
		}(index)
	}
	wait.Wait()
	close(errors)
	for processErr := range errors {
		t.Fatal(processErr)
	}
	for index := 0; index < 100; index++ {
		if _, err := manager.Process(context.Background(), Incoming{
			Sequence: uint64(index), ServerMessageID: fmt.Sprintf("burst-%04d", index),
			GroupID: 7, UserID: 1, Kind: groupmgr.MessageText, Text: "重复消息", SentAt: time.Now(),
		}); err != nil {
			t.Fatal(err)
		}
	}
	var count int
	if err := database.DB.QueryRowContext(context.Background(), `SELECT COUNT(*) FROM messages WHERE account_id=? AND group_id=?`, "9001", 7).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if count != 1000 {
		t.Fatalf("message count=%d, want 1000", count)
	}
}

func TestDisabledGroupMessageIsStoredWithoutActions(t *testing.T) {
	database, err := store.Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	gateway := &fakeGateway{groups: []groupmgr.Group{{GroupID: 7, Name: "停用群"}}}
	manager := New(database, gateway, "9001")
	if err := manager.SyncGroups(context.Background()); err != nil {
		t.Fatal(err)
	}
	result, err := manager.Process(context.Background(), Incoming{Sequence: 1, ServerMessageID: "disabled", GroupID: 7, UserID: 8, Kind: groupmgr.MessageText, Text: "只保存", SentAt: time.Now()})
	if err != nil || !result.Stored || result.Actions != 0 {
		t.Fatalf("result=%+v err=%v", result, err)
	}
	stored, err := database.GetMessage(context.Background(), "9001", 7, "disabled")
	if err != nil || stored.ProcessedAt == nil {
		t.Fatalf("stored=%+v err=%v", stored, err)
	}
	unknown, err := manager.Process(context.Background(), Incoming{Sequence: 2, ServerMessageID: "unknown", GroupID: 99, UserID: 8, Kind: groupmgr.MessageText, Text: "跳过", SentAt: time.Now()})
	if err != nil || unknown.Stored {
		t.Fatalf("unknown=%+v err=%v", unknown, err)
	}
}
