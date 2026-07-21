use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub display_name: String,
    pub role: String,
    pub discovered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub account_id: String,
    pub group_id: i64,
    pub name: String,
    pub owner_user_id: i64,
    pub enabled: bool,
    pub ai_enabled: bool,
    pub moderation_enabled: bool,
    pub manual_takeover: bool,
    pub welcome_message: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub account_id: String,
    pub group_id: i64,
    pub user_id: i64,
    pub nim_id: String,
    pub nickname: String,
    pub card_name: String,
    pub original_card_name: String,
    pub managed_card_name: String,
    pub card_suffix: String,
    pub role: String,
    pub account_state: String,
    pub blacklisted: bool,
    pub present: bool,
    /// 来源标记：baseline、online-joined、offline-discovered、message-discovered。
    pub join_source: String,
    /// 管理员是否已查看该成员的新增/状态提示。
    pub prompt_read: bool,
    pub locked_card_name: String,
    pub violation_count: i64,
    pub discovered_at: DateTime<Utc>,
    pub joined_at: Option<DateTime<Utc>>,
    pub last_seen_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberRef {
    pub user_id: Option<i64>,
    pub nim_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberRoster {
    pub members: Vec<Member>,
    pub reported_count: usize,
    pub resolved_count: usize,
    pub complete: bool,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub server_message_id: String,
    pub sequence: i64,
    pub user_id: i64,
    pub sender_name: String,
    pub kind: String,
    pub text: String,
    pub sent_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub processing_state: String,
    pub attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedMessage {
    pub id: i64,
    pub inserted: bool,
    pub processed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleAction {
    pub kind: String,
    pub duration_seconds: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModerationRule {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub name: String,
    pub matcher: String,
    pub pattern: String,
    pub threshold: i64,
    pub count: i64,
    pub window_seconds: i64,
    pub cooldown_seconds: i64,
    pub priority: i64,
    pub mode: String,
    pub enabled: bool,
    pub semantic_threshold: f64,
    pub exempt_roles: Vec<String>,
    pub exempt_user_ids: Vec<i64>,
    pub actions: Vec<RuleAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBase {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub built_in: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocument {
    pub id: i64,
    pub base_id: i64,
    pub base_name: String,
    pub title: String,
    pub kind: String,
    pub content: String,
    pub source: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBinding {
    pub base_id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GroupSchedule {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub enabled: bool,
    pub open_time: String,
    pub close_time: String,
    pub timezone: String,
    pub group_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRun {
    pub id: i64,
    pub schedule_id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub local_date: String,
    pub action: String,
    pub run_key: String,
    pub success: bool,
    pub error: String,
    pub attempts: i64,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GroupAiPermissions {
    pub account_id: String,
    pub group_id: i64,
    pub reply: bool,
    pub tasks: bool,
    pub recall: bool,
    pub mute: bool,
    pub remove: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub title: String,
    pub description: String,
    pub status: String,
    pub assignee_id: i64,
    pub created_by: i64,
    pub due_at: Option<DateTime<Utc>>,
    pub reminder_at: Option<DateTime<Utc>>,
    pub reminder_sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailySummary {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub local_date: String,
    pub content: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub user_id: i64,
    pub actor: String,
    pub event: String,
    pub level: String,
    pub details: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionRecord {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub user_id: i64,
    pub message_id: Option<i64>,
    pub rule_id: Option<i64>,
    pub kind: String,
    pub mode: String,
    pub duration_seconds: i64,
    pub reason: String,
    pub success: bool,
    pub error: String,
    pub dedupe_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CardPlan {
    pub member: Member,
    pub original_name: String,
    pub suggested_name: String,
    pub suffix: String,
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CardRenameJob {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub user_id: i64,
    pub nim_id: String,
    pub original_name: String,
    pub desired_name: String,
    pub suffix: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: String,
    pub welcome_pending: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CardPreview {
    pub group_id: i64,
    pub prefix: String,
    pub items: Vec<CardPlan>,
    pub will_rename: usize,
    pub already_managed: usize,
    pub excluded: usize,
    pub conflicts: usize,
    pub missing_identity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiDecision {
    pub reply: String,
    pub actions: Vec<RuleAction>,
    pub tasks: Vec<AiTask>,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiTask {
    pub title: String,
    pub description: String,
    pub due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PredictionSnapshot {
    pub game: String,
    pub period: String,
    pub result: Vec<i64>,
    pub updated_at: DateTime<Utc>,
    pub history: Vec<Vec<i64>>,
    pub freshness: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PredictionResult {
    pub game: String,
    pub period: String,
    pub latest_result: Vec<i64>,
    pub trend: String,
    pub candidates: Vec<i64>,
    pub confidence: f64,
    pub updated_at: DateTime<Utc>,
    pub freshness: String,
}
