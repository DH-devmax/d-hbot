// Package moderation evaluates deterministic and semantic group moderation
// rules. It deliberately contains no transport or persistence code so the
// engine can be used by the message listener, tests, and the native UI.
package moderation

import "time"

// MatcherType identifies the value a rule evaluates.
type MatcherType string

const (
	MatcherExact       MatcherType = "exact"
	MatcherContains    MatcherType = "contains"
	MatcherPrefix      MatcherType = "prefix"
	MatcherRegex       MatcherType = "regex"
	MatcherLength      MatcherType = "length"
	MatcherLines       MatcherType = "lines"
	MatcherImageCount  MatcherType = "image_count"
	MatcherSemantic    MatcherType = "semantic"
	MatcherRenameCount MatcherType = "rename_count"
	MatcherBlacklist   MatcherType = "blacklist"
)

// RuleMode controls whether a matched rule can produce executable actions.
type RuleMode string

const (
	ModeDryRun  RuleMode = "dry-run"
	ModeObserve RuleMode = "observe"
	ModeAuto    RuleMode = "auto"
)

// MemberRole is intentionally a small stable set. Unknown roles are retained
// by callers as RoleUnknown and never receive an exemption accidentally.
type MemberRole string

const (
	RoleUnknown MemberRole = "unknown"
	RoleMember  MemberRole = "member"
	RoleAdmin   MemberRole = "admin"
	RoleOwner   MemberRole = "owner"
	RoleBot     MemberRole = "bot"
)

// MessageKind describes the message payload relevant to moderation. A text
// message can be accompanied by an image marker when the gateway only exposes
// the message type, which is why image is separate from text content.
type MessageKind string

const (
	MessageText  MessageKind = "text"
	MessageImage MessageKind = "image"
	MessageOther MessageKind = "other"
)

// RuleActionType is an action requested by a rule. The engine does not execute
// these actions; a gateway/worker is responsible for applying them.
type RuleActionType string

const (
	ActionReply     RuleActionType = "reply"
	ActionRecall    RuleActionType = "recall"
	ActionMute      RuleActionType = "mute"
	ActionRemove    RuleActionType = "remove"
	ActionBlacklist RuleActionType = "blacklist"
	ActionNotify    RuleActionType = "notify"
	ActionRename    RuleActionType = "rename"
)

// RuleAction is the serialisable action configuration stored with a rule.
type RuleAction struct {
	Type     RuleActionType `json:"type"`
	Duration time.Duration  `json:"duration,omitempty"`
	Reply    string         `json:"reply,omitempty"`
	Nickname string         `json:"nickname,omitempty"`
	Message  string         `json:"message,omitempty"`
}

// ModerationRule is compatible with the group manager model: callers can copy
// fields directly without importing this package from their persistence layer.
type ModerationRule struct {
	ID                int64         `json:"id"`
	GroupID           int64         `json:"groupId"`
	Name              string        `json:"name"`
	Matcher           MatcherType   `json:"matcher"`
	Pattern           string        `json:"pattern,omitempty"`
	Threshold         int           `json:"threshold,omitempty"`
	Window            time.Duration `json:"window,omitempty"`
	Count             int           `json:"count,omitempty"`
	Cooldown          time.Duration `json:"cooldown,omitempty"`
	Priority          int           `json:"priority,omitempty"`
	Mode              RuleMode      `json:"mode"`
	Enabled           bool          `json:"enabled"`
	SemanticThreshold float64       `json:"semanticThreshold,omitempty"`
	ExemptRoles       []MemberRole  `json:"exemptRoles,omitempty"`
	ExemptUserIDs     []int64       `json:"exemptUserIds,omitempty"`
	Actions           []RuleAction  `json:"actions"`
}

// Rule is retained as a short alias for integrations that use Rule in their
// public API.
type Rule = ModerationRule

// Message is the input event consumed by Engine.Evaluate. SentAt is used for
// all windows and cooldowns; a zero value uses time.Now().UTC().
type Message struct {
	AccountID        string             `json:"accountId"`
	GroupID          int64              `json:"groupId"`
	UserID           int64              `json:"userId"`
	ServerMessageID  string             `json:"serverMessageId"`
	Sequence         int64              `json:"sequence"`
	Kind             MessageKind        `json:"kind"`
	Text             string             `json:"text"`
	SentAt           time.Time          `json:"sentAt"`
	Role             MemberRole         `json:"role"`
	SemanticScore    float64            `json:"semanticScore,omitempty"`
	SemanticScores   map[string]float64 `json:"semanticScores,omitempty"`
	Blacklisted      bool               `json:"blacklisted,omitempty"`
	NicknameChanged  bool               `json:"nicknameChanged,omitempty"`
	PriorRenameCount int                `json:"priorRenameCount,omitempty"`
	OriginalNickname string             `json:"originalNickname,omitempty"`
	CurrentNickname  string             `json:"currentNickname,omitempty"`
}

// Action is a concrete action emitted by evaluation. RuleID/Priority identify
// the strongest source for audit and troubleshooting.
type Action struct {
	Type     RuleActionType `json:"type"`
	Duration time.Duration  `json:"duration,omitempty"`
	Reply    string         `json:"reply,omitempty"`
	Nickname string         `json:"nickname,omitempty"`
	Message  string         `json:"message,omitempty"`
	RuleID   int64          `json:"ruleId,omitempty"`
	Priority int            `json:"priority,omitempty"`
}

// RuleMatch records one rule's result before actions are merged.
type RuleMatch struct {
	Rule       ModerationRule `json:"rule"`
	Matched    bool           `json:"matched"`
	Suppressed bool           `json:"suppressed,omitempty"`
	Reason     string         `json:"reason,omitempty"`
	Actions    []Action       `json:"actions,omitempty"`
}

// Evaluation contains both the audit/planning actions and actions permitted to
// execute. Actions includes observe and dry-run matches; ExecutableActions only
// includes unsuppressed auto-mode matches.
type Evaluation struct {
	Message           Message     `json:"message"`
	Matches           []RuleMatch `json:"matches,omitempty"`
	Actions           []Action    `json:"actions,omitempty"`
	ExecutableActions []Action    `json:"executableActions,omitempty"`
	Error             string      `json:"error,omitempty"`
}
