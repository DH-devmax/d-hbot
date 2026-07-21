package moderation

import (
	"os"
	"strings"
	"testing"
	"time"

	"dh/internal/groupmgr"
)

func TestTextAndSemanticMatchers(t *testing.T) {
	rules := []ModerationRule{
		testRule(1, MatcherExact, "精确"),
		testRule(2, MatcherContains, "包含"),
		testRule(3, MatcherPrefix, "开头"),
		{ID: 4, GroupID: 7, Matcher: MatcherRegex, Pattern: `订单\d+`, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionNotify, Message: "regex"}}},
		{ID: 5, GroupID: 7, Matcher: MatcherLength, Threshold: 3, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionNotify, Message: "length"}}},
		{ID: 6, GroupID: 7, Matcher: MatcherLines, Threshold: 2, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionNotify, Message: "lines"}}},
		{ID: 7, GroupID: 7, Matcher: MatcherSemantic, Pattern: "scam", SemanticThreshold: .8, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionNotify, Message: "semantic"}}},
	}
	e := NewEngine(rules)
	cases := []struct {
		name    string
		message Message
		ruleID  int64
	}{
		{"exact", Message{GroupID: 7, UserID: 1, Text: "精确"}, 1},
		{"contains", Message{GroupID: 7, UserID: 1, Text: "这里包含关键词"}, 2},
		{"prefix", Message{GroupID: 7, UserID: 1, Text: "开头后续"}, 3},
		{"regex", Message{GroupID: 7, UserID: 1, Text: "订单123"}, 4},
		{"length", Message{GroupID: 7, UserID: 1, Text: "汉aa"}, 5},
		{"lines", Message{GroupID: 7, UserID: 1, Text: "一\n二\n三"}, 6},
		{"semantic", Message{GroupID: 7, UserID: 1, SemanticScores: map[string]float64{"scam": .91}}, 7},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := e.Evaluate(tc.message)
			if !matchedRule(got, tc.ruleID) {
				t.Fatalf("rule %d did not match: %#v", tc.ruleID, got.Matches)
			}
		})
	}
}

func TestZCGWeightAndDefaultThresholds(t *testing.T) {
	if got := WeightedLength("汉A🙂"); got != 4 {
		t.Fatalf("WeightedLength = %d, want 4", got)
	}
	if got := LineCount("一\n二\n三"); got != 3 {
		t.Fatalf("LineCount = %d, want 3", got)
	}
	e := NewEngine(DefaultZCGRules(7))
	boundary := e.Evaluate(Message{GroupID: 7, UserID: 2, Text: strings.Repeat("a", 100), SentAt: time.Unix(1, 0)})
	if len(boundary.ExecutableActions) != 0 {
		t.Fatalf("exact threshold matched: %#v", boundary.ExecutableActions)
	}
	muted := e.Evaluate(Message{GroupID: 7, UserID: 3, Text: strings.Repeat("a", 101), SentAt: time.Unix(2, 0)})
	assertActionTypes(t, muted.ExecutableActions, ActionRecall)
	removed := e.Evaluate(Message{GroupID: 7, UserID: 4, Text: strings.Repeat("汉", 101), SentAt: time.Unix(3, 0)})
	assertActionTypes(t, removed.ExecutableActions, ActionRecall)
	otherGroup := e.Evaluate(Message{GroupID: 8, UserID: 4, Text: strings.Repeat("汉", 101), SentAt: time.Unix(4, 0)})
	if len(otherGroup.ExecutableActions) != 0 {
		t.Fatalf("rules leaked to another group: %#v", otherGroup.ExecutableActions)
	}
	admin := e.Evaluate(Message{GroupID: 7, UserID: 5, Role: RoleAdmin, Text: strings.Repeat("汉", 101), SentAt: time.Unix(5, 0)})
	if len(admin.ExecutableActions) != 0 {
		t.Fatalf("default admin exemption produced actions: %#v", admin.ExecutableActions)
	}
}

func TestImageWindowAndIsolation(t *testing.T) {
	e := NewEngine(DefaultZCGRules(7))
	base := time.Unix(1000, 0)
	first := e.Evaluate(Message{AccountID: "a", GroupID: 7, UserID: 3, Kind: MessageImage, SentAt: base})
	assertActionTypes(t, first.ExecutableActions, ActionRecall)
	e.Evaluate(Message{AccountID: "a", GroupID: 7, UserID: 3, Kind: MessageImage, SentAt: base.Add(5 * time.Minute)})
	third := e.Evaluate(Message{AccountID: "a", GroupID: 7, UserID: 3, Kind: MessageImage, SentAt: base.Add(10 * time.Minute)})
	assertActionTypes(t, third.ExecutableActions, ActionRecall)

	isolated := e.Evaluate(Message{AccountID: "a", GroupID: 7, UserID: 4, Kind: MessageImage, SentAt: base.Add(10 * time.Minute)})
	assertActionTypes(t, isolated.ExecutableActions, ActionRecall)
	differentAccount := e.Evaluate(Message{AccountID: "b", GroupID: 7, UserID: 3, Kind: MessageImage, SentAt: base.Add(10 * time.Minute)})
	assertActionTypes(t, differentAccount.ExecutableActions, ActionRecall)

	// A late old event must not count messages whose event timestamps are in its future.
	outOfOrder := NewEngine(DefaultZCGRules(7))
	outOfOrder.Evaluate(Message{GroupID: 7, UserID: 5, Kind: MessageImage, SentAt: base.Add(5 * time.Minute)})
	outOfOrder.Evaluate(Message{GroupID: 7, UserID: 5, Kind: MessageImage, SentAt: base.Add(6 * time.Minute)})
	lateOld := outOfOrder.Evaluate(Message{GroupID: 7, UserID: 5, Kind: MessageImage, SentAt: base})
	assertActionTypes(t, lateOld.Actions, ActionRecall)
	if len(lateOld.ExecutableActions) != 0 {
		t.Fatalf("late old image bypassed cooldown: %#v", lateOld.ExecutableActions)
	}
}

func TestRenameCounterOnlyRecalls(t *testing.T) {
	e := NewEngine(DefaultZCGRules(7))
	base := time.Unix(1000, 0)
	for i := 0; i < 4; i++ {
		got := e.Evaluate(Message{GroupID: 7, UserID: 9, NicknameChanged: true, OriginalNickname: "固定名", SentAt: base.Add(time.Duration(i) * time.Hour)})
		assertActionTypes(t, got.ExecutableActions, ActionRecall)
	}
	fifth := e.Evaluate(Message{GroupID: 7, UserID: 9, NicknameChanged: true, OriginalNickname: "固定名", SentAt: base.Add(30 * 24 * time.Hour)})
	assertActionTypes(t, fifth.ExecutableActions, ActionRecall)

	restarted := NewEngine(DefaultZCGRules(7))
	fromStorage := restarted.Evaluate(Message{GroupID: 7, UserID: 10, NicknameChanged: true, PriorRenameCount: 4, OriginalNickname: "固定名", SentAt: base})
	assertActionTypes(t, fromStorage.ExecutableActions, ActionRecall)
}

func TestExemptionCooldownAndModes(t *testing.T) {
	rule := ModerationRule{
		ID: 1, GroupID: 1, Matcher: MatcherContains, Pattern: "广告", Mode: ModeAuto, Enabled: true,
		Cooldown: time.Hour, ExemptRoles: []MemberRole{RoleAdmin}, ExemptUserIDs: []int64{99},
		Actions: []RuleAction{{Type: ActionMute}},
	}
	e := NewEngine(ruleSet(rule))
	admin := e.Evaluate(Message{GroupID: 1, UserID: 1, Role: RoleAdmin, Text: "广告", SentAt: time.Unix(1, 0)})
	if len(admin.ExecutableActions) != 0 {
		t.Fatal("admin exemption produced an action")
	}
	member := Message{GroupID: 1, UserID: 2, Text: "广告", SentAt: time.Unix(2, 0)}
	assertActionTypes(t, e.Evaluate(member).ExecutableActions, ActionMute)
	member.SentAt = member.SentAt.Add(time.Minute)
	second := e.Evaluate(member)
	if len(second.ExecutableActions) != 0 || !second.Matches[0].Suppressed {
		t.Fatalf("cooldown did not suppress: %#v", second)
	}
	member.SentAt = member.SentAt.Add(time.Hour)
	assertActionTypes(t, e.Evaluate(member).ExecutableActions, ActionMute)
	member.SentAt = time.Unix(1, 0)
	if got := e.Evaluate(member); len(got.ExecutableActions) != 0 {
		t.Fatalf("late old message bypassed cooldown: %#v", got.ExecutableActions)
	}

	modes := []ModerationRule{
		{ID: 2, GroupID: 1, Matcher: MatcherExact, Pattern: "x", Mode: ModeDryRun, Enabled: true, Actions: []RuleAction{{Type: ActionRecall}}},
		{ID: 3, GroupID: 1, Matcher: MatcherExact, Pattern: "x", Mode: ModeObserve, Enabled: true, Actions: []RuleAction{{Type: ActionBlacklist}}},
	}
	preview := NewEngine(modes).Evaluate(Message{GroupID: 1, UserID: 2, Text: "x"})
	assertActionTypes(t, preview.Actions, ActionRecall, ActionBlacklist)
	if len(preview.ExecutableActions) != 0 {
		t.Fatalf("preview modes became executable: %#v", preview.ExecutableActions)
	}
}

func TestMergeStrengthPriorityAndIndependentUnion(t *testing.T) {
	rules := []ModerationRule{
		{ID: 1, GroupID: 1, Matcher: MatcherContains, Pattern: "x", Priority: 1, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionReply, Reply: "low"}, {Type: ActionMute, Duration: time.Minute}, {Type: ActionRecall}, {Type: ActionNotify, Message: "n1"}}},
		{ID: 2, GroupID: 1, Matcher: MatcherContains, Pattern: "x", Priority: 2, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionRemove}, {Type: ActionBlacklist}, {Type: ActionNotify, Message: "n2"}, {Type: ActionNotify, Message: "n1"}}},
	}
	got := NewEngine(rules).Evaluate(Message{GroupID: 1, UserID: 2, Text: "x"})
	assertActionTypes(t, got.ExecutableActions, ActionRecall, ActionRemove, ActionBlacklist, ActionNotify, ActionNotify)
	for _, action := range got.ExecutableActions {
		if action.Type == ActionMute || action.Type == ActionReply {
			t.Fatalf("weaker action survived remove: %#v", got.ExecutableActions)
		}
	}

	replies := []ModerationRule{
		{ID: 3, GroupID: 1, Matcher: MatcherExact, Pattern: "y", Priority: 1, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionReply, Reply: "low"}}},
		{ID: 4, GroupID: 1, Matcher: MatcherExact, Pattern: "y", Priority: 9, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionReply, Reply: "high"}}},
	}
	reply := NewEngine(replies).Evaluate(Message{GroupID: 1, UserID: 2, Text: "y"})
	assertActionTypes(t, reply.ExecutableActions, ActionReply)
	if reply.ExecutableActions[0].Reply != "high" {
		t.Fatalf("priority reply = %q", reply.ExecutableActions[0].Reply)
	}
}

func TestSemanticDefaultsObserveOnly(t *testing.T) {
	e := NewEngine(DefaultSemanticRules(7))
	got := e.Evaluate(Message{GroupID: 7, UserID: 2, SemanticScores: map[string]float64{"scam": .95}})
	assertActionTypes(t, got.Actions, ActionRecall)
	if len(got.ExecutableActions) != 0 {
		t.Fatalf("semantic default executed automatically: %#v", got.ExecutableActions)
	}
}

func TestDefaultTemplatesLeaveIDsToStorage(t *testing.T) {
	for _, rule := range append(DefaultZCGRules(1), DefaultZCGRules(2)...) {
		if rule.ID != 0 {
			t.Fatalf("default rule %q has persistent ID %d", rule.Name, rule.ID)
		}
	}
}

func TestValidationAndGroupManagerAdapter(t *testing.T) {
	if _, err := NewValidatedEngine([]ModerationRule{{ID: 1, Matcher: MatcherRegex, Pattern: "[", Mode: ModeAuto, Enabled: true}}); err == nil {
		t.Fatal("invalid regex accepted")
	}
	modelRule := groupmgr.ModerationRule{
		ID: 3, GroupID: 8, Matcher: groupmgr.MatcherContains, Pattern: "spam", Mode: groupmgr.RuleAuto, Enabled: true,
		ExemptRoles: []groupmgr.MemberRole{groupmgr.RoleOwner}, Actions: []groupmgr.RuleAction{{Type: groupmgr.ActionReply, Message: "stop"}},
	}
	rule := RuleFromModel(modelRule)
	if rule.Matcher != MatcherContains || rule.Actions[0].Reply != "stop" || rule.ExemptRoles[0] != RoleOwner {
		t.Fatalf("rule conversion = %#v", rule)
	}
	modelMessage := groupmgr.Message{AccountID: "acct", GroupID: 8, UserID: 4, Kind: groupmgr.MessageText, Text: "spam", SentAt: time.Unix(10, 0)}
	member := groupmgr.Member{Role: groupmgr.RoleMember, Blacklisted: true, LockedCardName: "locked", RenameViolations: 4}
	message := MessageFromModel(modelMessage, &member, map[string]float64{"spam": .9})
	if message.Role != RoleMember || !message.Blacklisted || message.OriginalNickname != "locked" || message.PriorRenameCount != 4 {
		t.Fatalf("message conversion = %#v", message)
	}
}

func TestRulesJSONRoundTripAndRejectsUnknownFields(t *testing.T) {
	rules := DefaultSemanticRules(7)
	data, err := ExportRules(rules)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := ImportRules(data)
	if err != nil {
		t.Fatal(err)
	}
	if len(decoded) != len(rules) || decoded[0].GroupID != 7 {
		t.Fatalf("decoded rules = %#v", decoded)
	}
	if _, err := ImportRules([]byte(`[{"id":1,"matcher":"blacklist","mode":"auto","enabled":true,"actions":[],"unexpected":1}]`)); err == nil {
		t.Fatal("unknown JSON field accepted")
	}
}

func TestPackagedZCGTemplateIsImportable(t *testing.T) {
	raw, err := os.ReadFile("../../package/ZCG-Compatible-Rules.json")
	if err != nil {
		t.Fatal(err)
	}
	rules, err := ImportRules(raw)
	if err != nil {
		t.Fatal(err)
	}
	if len(rules) != 11 {
		t.Fatalf("packaged rule count = %d, want 11", len(rules))
	}
	for _, rule := range rules {
		if rule.GroupID != 0 || len(rule.Actions) != 1 || rule.Actions[0].Type != ActionRecall {
			t.Fatalf("packaged rule is not global recall-only: %+v", rule)
		}
	}
}

func testRule(id int64, matcher MatcherType, pattern string) ModerationRule {
	return ModerationRule{ID: id, GroupID: 7, Matcher: matcher, Pattern: pattern, Mode: ModeAuto, Enabled: true, Actions: []RuleAction{{Type: ActionNotify, Message: pattern}}}
}

func ruleSet(rules ...ModerationRule) []ModerationRule { return rules }

func matchedRule(got Evaluation, ruleID int64) bool {
	for _, match := range got.Matches {
		if match.Rule.ID == ruleID && match.Matched {
			return true
		}
	}
	return false
}

func assertActionTypes(t *testing.T, got []Action, want ...RuleActionType) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("action count = %d, want %d: %#v", len(got), len(want), got)
	}
	for i := range want {
		if got[i].Type != want[i] {
			t.Fatalf("action %d = %s, want %s: %#v", i, got[i].Type, want[i], got)
		}
	}
}
