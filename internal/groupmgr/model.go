package groupmgr

import "time"

type MemberRole string

const (
	RoleMember MemberRole = "member"
	RoleAdmin  MemberRole = "admin"
	RoleOwner  MemberRole = "owner"
	RoleBot    MemberRole = "bot"
)

type MessageKind string

const (
	MessageText   MessageKind = "text"
	MessageImage  MessageKind = "image"
	MessageCard   MessageKind = "card"
	MessageNotice MessageKind = "notice"
	MessageOther  MessageKind = "other"
)

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

type RuleMode string

const (
	RuleDryRun  RuleMode = "dry-run"
	RuleObserve RuleMode = "observe"
	RuleAuto    RuleMode = "auto"
)

type ActionType string

const (
	ActionReply     ActionType = "reply"
	ActionMute      ActionType = "mute"
	ActionRemove    ActionType = "remove"
	ActionRecall    ActionType = "recall"
	ActionBlacklist ActionType = "blacklist"
	ActionNotify    ActionType = "notify"
	ActionRename    ActionType = "rename"
	ActionUnmute    ActionType = "unmute"
	ActionGroupMute ActionType = "group_mute"
)

type Group struct {
	AccountID         string    `json:"accountId"`
	GroupID           int64     `json:"groupId"`
	Name              string    `json:"name"`
	OwnerUserID       int64     `json:"ownerUserId,omitempty"`
	Enabled           bool      `json:"enabled"`
	AIEnabled         bool      `json:"aiEnabled"`
	ModerationEnabled bool      `json:"moderationEnabled"`
	ManualTakeover    bool      `json:"manualTakeover"`
	TaskReminders     bool      `json:"taskReminders"`
	WelcomeMessage    string    `json:"welcomeMessage,omitempty"`
	SummarySchedule   string    `json:"summarySchedule,omitempty"`
	CreatedAt         time.Time `json:"createdAt"`
	UpdatedAt         time.Time `json:"updatedAt"`
	SyncedAt          time.Time `json:"syncedAt,omitempty"`
}

// GroupSchedule describes the daily speaking-state policy for one or more
// managed groups. Times are stored as HH:MM and evaluated in the local OS
// timezone so a portable database behaves consistently on each machine.
type GroupSchedule struct {
	ID        int64     `json:"id"`
	AccountID string    `json:"accountId"`
	Name      string    `json:"name"`
	Enabled   bool      `json:"enabled"`
	OpenTime  string    `json:"openTime"`
	CloseTime string    `json:"closeTime"`
	Timezone  string    `json:"timezone"`
	GroupIDs  []int64   `json:"groupIds"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
}

type ScheduleRun struct {
	ID          int64      `json:"id"`
	ScheduleID  int64      `json:"scheduleId"`
	AccountID   string     `json:"accountId"`
	GroupID     int64      `json:"groupId"`
	LocalDate   string     `json:"localDate"`
	Action      ActionType `json:"action"`
	RunKey      string     `json:"runKey"`
	Success     bool       `json:"success"`
	Error       string     `json:"error,omitempty"`
	Attempts    int        `json:"attempts"`
	NextRetryAt time.Time  `json:"nextRetryAt,omitempty"`
	CreatedAt   time.Time  `json:"createdAt"`
}

type ScheduleBinding struct {
	ScheduleID int64     `json:"scheduleId"`
	AccountID  string    `json:"accountId"`
	GroupID    int64     `json:"groupId"`
	Enabled    bool      `json:"enabled"`
	CreatedAt  time.Time `json:"createdAt"`
}

type Member struct {
	AccountID        string     `json:"accountId"`
	GroupID          int64      `json:"groupId"`
	UserID           int64      `json:"userId"`
	NIMID            string     `json:"nimId,omitempty"`
	Nickname         string     `json:"nickname"`
	CardName         string     `json:"cardName,omitempty"`
	AccountState     string     `json:"accountState,omitempty"`
	OriginalCardName string     `json:"originalCardName,omitempty"`
	ManagedCardName  string     `json:"managedCardName,omitempty"`
	CardSuffix       string     `json:"cardSuffix,omitempty"`
	Role             MemberRole `json:"role"`
	Blacklisted      bool       `json:"blacklisted"`
	Present          bool       `json:"present"`
	CardStatus       CardStatus `json:"cardStatus,omitempty"`
	JoinSource       JoinSource `json:"joinSource,omitempty"`
	NoticeRead       bool       `json:"noticeRead"`
	LockedCardName   string     `json:"lockedCardName,omitempty"`
	RenameViolations int        `json:"renameViolations"`
	JoinedAt         time.Time  `json:"joinedAt,omitempty"`
	DiscoveredAt     time.Time  `json:"discoveredAt,omitempty"`
	LastSeenAt       time.Time  `json:"lastSeenAt,omitempty"`
	UpdatedAt        time.Time  `json:"updatedAt"`
}

type CardStatus string

const (
	CardUnmanaged CardStatus = "unmanaged"
	CardPlanned   CardStatus = "planned"
	CardQueued    CardStatus = "queued"
	CardRunning   CardStatus = "running"
	CardVerified  CardStatus = "verified"
	CardFailed    CardStatus = "failed"
	CardExcluded  CardStatus = "excluded"
	CardConflict  CardStatus = "conflict"
)

type JoinSource string

const (
	JoinBaseline JoinSource = "baseline"
	JoinOnline   JoinSource = "online"
	JoinOffline  JoinSource = "offline"
	JoinMessage  JoinSource = "message"
)

type GroupCardSettings struct {
	AccountID      string    `json:"accountId"`
	GroupID        int64     `json:"groupId"`
	Prefix         string    `json:"prefix"`
	AutoRename     bool      `json:"autoRename"`
	Paused         bool      `json:"paused"`
	BaselineAt     time.Time `json:"baselineAt,omitempty"`
	LastSnapshotAt time.Time `json:"lastSnapshotAt,omitempty"`
	ReportedCount  int       `json:"reportedCount"`
	ResolvedCount  int       `json:"resolvedCount"`
	RosterComplete bool      `json:"rosterComplete"`
	UpdatedAt      time.Time `json:"updatedAt"`
}

type RenameJobState string

const (
	RenameQueued   RenameJobState = "queued"
	RenameRunning  RenameJobState = "running"
	RenameVerified RenameJobState = "verified"
	RenameFailed   RenameJobState = "failed"
	RenamePaused   RenameJobState = "paused"
	RenameCanceled RenameJobState = "canceled"
)

type CardRenameJob struct {
	ID             int64          `json:"id"`
	AccountID      string         `json:"accountId"`
	GroupID        int64          `json:"groupId"`
	UserID         int64          `json:"userId"`
	NIMID          string         `json:"nimId,omitempty"`
	OriginalName   string         `json:"originalName,omitempty"`
	DesiredName    string         `json:"desiredName"`
	Suffix         string         `json:"suffix,omitempty"`
	Source         JoinSource     `json:"source"`
	State          RenameJobState `json:"state"`
	Attempts       int            `json:"attempts"`
	NextAttemptAt  time.Time      `json:"nextAttemptAt,omitempty"`
	LastError      string         `json:"lastError,omitempty"`
	WelcomePending bool           `json:"welcomePending"`
	CreatedAt      time.Time      `json:"createdAt"`
	UpdatedAt      time.Time      `json:"updatedAt"`
}

type CardPreview struct {
	GroupID         int64      `json:"groupId"`
	Prefix          string     `json:"prefix"`
	Items           []CardPlan `json:"items"`
	WillRename      int        `json:"willRename"`
	AlreadyManaged  int        `json:"alreadyManaged"`
	Excluded        int        `json:"excluded"`
	Conflicts       int        `json:"conflicts"`
	MissingIdentity int        `json:"missingIdentity"`
	CoverageGap     int        `json:"coverageGap"`
}

type CardPlan struct {
	Member        Member     `json:"member"`
	OriginalName  string     `json:"originalName,omitempty"`
	SuggestedName string     `json:"suggestedName,omitempty"`
	Suffix        string     `json:"suffix,omitempty"`
	Status        CardStatus `json:"status"`
	Reason        string     `json:"reason,omitempty"`
}

type Message struct {
	ID              int64       `json:"id"`
	AccountID       string      `json:"accountId"`
	GroupID         int64       `json:"groupId"`
	ServerMessageID string      `json:"serverMessageId"`
	Sequence        int64       `json:"sequence"`
	UserID          int64       `json:"userId"`
	SenderName      string      `json:"senderName,omitempty"`
	Kind            MessageKind `json:"kind"`
	Text            string      `json:"text,omitempty"`
	SentAt          time.Time   `json:"sentAt"`
	ReceivedAt      time.Time   `json:"receivedAt"`
	ProcessedAt     *time.Time  `json:"processedAt,omitempty"`
	AcknowledgedAt  *time.Time  `json:"acknowledgedAt,omitempty"`
}

type DailySummary struct {
	ID        int64     `json:"id"`
	AccountID string    `json:"accountId"`
	GroupID   int64     `json:"groupId"`
	Content   string    `json:"content"`
	Source    string    `json:"source"`
	CreatedAt time.Time `json:"createdAt"`
}

type RuleAction struct {
	Type     ActionType    `json:"type"`
	Duration time.Duration `json:"duration,omitempty"`
	Message  string        `json:"message,omitempty"`
}

type ModerationRule struct {
	ID                int64         `json:"id"`
	GroupID           int64         `json:"groupId"`
	Name              string        `json:"name"`
	Matcher           MatcherType   `json:"matcher"`
	Pattern           string        `json:"pattern,omitempty"`
	Threshold         int           `json:"threshold,omitempty"`
	Count             int           `json:"count,omitempty"`
	Window            time.Duration `json:"window,omitempty"`
	Cooldown          time.Duration `json:"cooldown,omitempty"`
	Priority          int           `json:"priority"`
	Mode              RuleMode      `json:"mode"`
	Enabled           bool          `json:"enabled"`
	SemanticThreshold float64       `json:"semanticThreshold,omitempty"`
	ExemptRoles       []MemberRole  `json:"exemptRoles,omitempty"`
	ExemptUserIDs     []int64       `json:"exemptUserIds,omitempty"`
	Actions           []RuleAction  `json:"actions"`
	CreatedAt         time.Time     `json:"createdAt"`
	UpdatedAt         time.Time     `json:"updatedAt"`
}

type Knowledge struct {
	ID        int64     `json:"id"`
	GroupID   int64     `json:"groupId"`
	Kind      string    `json:"kind"`
	Title     string    `json:"title"`
	Content   string    `json:"content"`
	Source    string    `json:"source,omitempty"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
}

type KnowledgeBase struct {
	ID          int64     `json:"id"`
	AccountID   string    `json:"accountId"`
	Name        string    `json:"name"`
	Description string    `json:"description,omitempty"`
	Enabled     bool      `json:"enabled"`
	BuiltIn     bool      `json:"builtIn"`
	ReadOnly    bool      `json:"readOnly"`
	CreatedAt   time.Time `json:"createdAt"`
	UpdatedAt   time.Time `json:"updatedAt"`
}

type KnowledgeDocument struct {
	ID          int64     `json:"id"`
	BaseID      int64     `json:"baseId"`
	BaseName    string    `json:"baseName,omitempty"`
	Title       string    `json:"title"`
	Kind        string    `json:"kind"`
	Content     string    `json:"content"`
	Source      string    `json:"source,omitempty"`
	ContentHash string    `json:"contentHash,omitempty"`
	CreatedAt   time.Time `json:"createdAt"`
	UpdatedAt   time.Time `json:"updatedAt"`
}

type KnowledgeBinding struct {
	BaseID    int64     `json:"baseId"`
	AccountID string    `json:"accountId"`
	GroupID   int64     `json:"groupId"`
	Enabled   bool      `json:"enabled"`
	CreatedAt time.Time `json:"createdAt,omitempty"`
}

type TaskStatus string

const (
	TaskPending   TaskStatus = "pending"
	TaskCompleted TaskStatus = "completed"
	TaskCancelled TaskStatus = "cancelled"
)

type Task struct {
	ID          int64      `json:"id"`
	GroupID     int64      `json:"groupId"`
	AssigneeID  int64      `json:"assigneeId,omitempty"`
	CreatedByID int64      `json:"createdById,omitempty"`
	Title       string     `json:"title"`
	Description string     `json:"description,omitempty"`
	Status      TaskStatus `json:"status"`
	DueAt       *time.Time `json:"dueAt,omitempty"`
	RemindedAt  *time.Time `json:"remindedAt,omitempty"`
	CreatedAt   time.Time  `json:"createdAt"`
	UpdatedAt   time.Time  `json:"updatedAt"`
}

type ActionRecord struct {
	ID        int64         `json:"id"`
	AccountID string        `json:"accountId"`
	GroupID   int64         `json:"groupId"`
	UserID    int64         `json:"userId"`
	MessageID int64         `json:"messageId,omitempty"`
	RuleID    int64         `json:"ruleId,omitempty"`
	Type      ActionType    `json:"type"`
	Mode      RuleMode      `json:"mode"`
	Duration  time.Duration `json:"duration,omitempty"`
	Reason    string        `json:"reason,omitempty"`
	Success   bool          `json:"success"`
	Error     string        `json:"error,omitempty"`
	CreatedAt time.Time     `json:"createdAt"`
}

type AuditEvent struct {
	ID        int64     `json:"id"`
	AccountID string    `json:"accountId"`
	GroupID   int64     `json:"groupId,omitempty"`
	UserID    int64     `json:"userId,omitempty"`
	Actor     string    `json:"actor"`
	Event     string    `json:"event"`
	Level     string    `json:"level"`
	Details   string    `json:"details,omitempty"`
	CreatedAt time.Time `json:"createdAt"`
}

type AppSetting struct {
	Key       string    `json:"key"`
	Value     string    `json:"value"`
	Sensitive bool      `json:"sensitive"`
	UpdatedAt time.Time `json:"updatedAt"`
}
