package moderation

import "dh/internal/groupmgr"

// RuleFromModel converts the persistence/group-manager representation into an
// executable rule without coupling groupmgr back to this package.
func RuleFromModel(rule groupmgr.ModerationRule) ModerationRule {
	actions := make([]RuleAction, 0, len(rule.Actions))
	for _, action := range rule.Actions {
		converted := RuleAction{
			Type: RuleActionType(action.Type), Duration: action.Duration, Message: action.Message,
		}
		if converted.Type == ActionReply {
			converted.Reply = action.Message
		}
		actions = append(actions, converted)
	}
	roles := make([]MemberRole, len(rule.ExemptRoles))
	for i, role := range rule.ExemptRoles {
		roles[i] = MemberRole(role)
	}
	return ModerationRule{
		ID: rule.ID, GroupID: rule.GroupID, Name: rule.Name,
		Matcher: MatcherType(rule.Matcher), Pattern: rule.Pattern,
		Threshold: rule.Threshold, Count: rule.Count, Window: rule.Window,
		Cooldown: rule.Cooldown, Priority: rule.Priority, Mode: RuleMode(rule.Mode),
		Enabled: rule.Enabled, SemanticThreshold: rule.SemanticThreshold,
		ExemptRoles: roles, ExemptUserIDs: append([]int64(nil), rule.ExemptUserIDs...),
		Actions: actions,
	}
}

// FromModel is the concise adapter name used by storage/processors.
func FromModel(rule groupmgr.ModerationRule) ModerationRule { return RuleFromModel(rule) }

// RulesFromModel converts a complete rule set while preserving configured
// order. Engine.SetRules applies priority ordering after validation.
func RulesFromModel(rules []groupmgr.ModerationRule) []ModerationRule {
	out := make([]ModerationRule, len(rules))
	for i, rule := range rules {
		out[i] = RuleFromModel(rule)
	}
	return out
}

// MessageFromModel combines a persisted message with its current member state
// and semantic classification. member may be nil when the member cache has not
// completed its initial sync.
func MessageFromModel(message groupmgr.Message, member *groupmgr.Member, semanticScores map[string]float64) Message {
	out := Message{
		AccountID: message.AccountID, GroupID: message.GroupID, UserID: message.UserID,
		ServerMessageID: message.ServerMessageID, Sequence: message.Sequence,
		Kind: MessageKind(message.Kind), Text: message.Text, SentAt: message.SentAt,
		SemanticScores: semanticScores,
	}
	if member != nil {
		out.Role = MemberRole(member.Role)
		out.Blacklisted = member.Blacklisted
		out.PriorRenameCount = member.RenameViolations
		out.OriginalNickname = member.LockedCardName
		out.CurrentNickname = member.CardName
	}
	return out
}
