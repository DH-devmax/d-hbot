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
    #[serde(default)]
    pub machine_rules_enabled: bool,
    #[serde(default)]
    pub ai_rules_enabled: bool,
    pub manual_takeover: bool,
    pub welcome_message: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GroupAnnouncement {
    pub group_id: i64,
    pub notice_id: String,
    pub content: String,
    pub mode: String,
    pub author_user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GroupMuteState {
    pub group_id: i64,
    pub muted: bool,
    pub source: String,
    pub checked_at: DateTime<Utc>,
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
pub enum RosterCompleteness {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberRoster {
    #[serde(default)]
    pub status: String,
    pub members: Vec<Member>,
    pub reported_count: usize,
    pub resolved_count: usize,
    pub complete: bool,
    pub completeness: RosterCompleteness,
    pub completeness_reason: String,
    pub http_returned_count: usize,
    pub http_reported_count: usize,
    pub http_cursor: Option<String>,
    pub nim_returned_count: usize,
    pub nim_reported_count: usize,
    pub nim_cursor: Option<String>,
    pub authority: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub source_errors: Vec<MemberSourceError>,
    #[serde(default)]
    pub retry_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub canonical_count: usize,
    #[serde(default)]
    pub synthetic_user_ids: Vec<i64>,
    #[serde(default)]
    pub http_pages: usize,
    #[serde(default)]
    pub nim_pages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemberSourceError {
    pub source: String,
    pub route: String,
    pub page: usize,
    pub cursor: Option<String>,
    pub reason: String,
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
    #[serde(default = "default_json_array")]
    pub mentions_json: String,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub flow: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedMessage {
    pub id: i64,
    pub inserted: bool,
    pub processed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayInboxEvent {
    pub account_id: String,
    pub bridge_session: String,
    pub bridge_sequence: i64,
    pub event_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayInboxItem {
    pub id: i64,
    pub account_id: String,
    pub bridge_session: String,
    pub bridge_sequence: i64,
    pub event_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: String,
    pub received_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub processed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BatchIngestResult {
    pub inserted: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutboxRequest {
    pub account_id: String,
    pub group_id: i64,
    pub effect_type: String,
    pub payload_json: String,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutboxItem {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub effect_type: String,
    pub payload_json: String,
    pub dedupe_key: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: String,
    pub receipt_json: String,
    pub created_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub priority: i64,
    pub lane: String,
    pub order_key: Option<String>,
    pub correlation_id: String,
    pub origin: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnqueuedEffect {
    pub id: i64,
    pub inserted: bool,
    pub state: String,
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
    #[serde(default = "default_machine_rule_type")]
    pub rule_type: String,
    #[serde(default = "default_global_scope")]
    pub scope: String,
    #[serde(default)]
    pub group_ids: Vec<i64>,
    #[serde(default = "default_medium_priority")]
    pub priority_level: String,
    #[serde(default)]
    pub whitelist_user_ids: Vec<i64>,
    // v1 JSON and pre-v9 database compatibility. These fields are not part of
    // the public v2 rule contract.
    #[serde(default, skip_serializing)]
    pub group_id: i64,
    pub name: String,
    pub matcher: String,
    pub pattern: String,
    pub threshold: i64,
    pub count: i64,
    pub window_seconds: i64,
    #[serde(default, skip_serializing)]
    pub cooldown_seconds: i64,
    #[serde(default, skip_serializing)]
    pub priority: i64,
    pub mode: String,
    pub enabled: bool,
    pub semantic_threshold: f64,
    #[serde(default, skip_serializing)]
    pub exempt_roles: Vec<String>,
    #[serde(default, skip_serializing)]
    pub exempt_user_ids: Vec<i64>,
    pub actions: Vec<RuleAction>,
}

impl ModerationRule {
    pub fn applies_to_group(&self, group_id: i64) -> bool {
        self.scope == "global" || self.group_ids.contains(&group_id)
    }

    pub fn priority_rank(&self) -> i64 {
        match self.priority_level.as_str() {
            "high" => 3,
            "low" => 1,
            _ => 2,
        }
    }

    pub fn is_ai_rule(&self) -> bool {
        self.rule_type == "ai"
    }
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
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunk {
    pub id: i64,
    pub document_id: i64,
    pub chunk_index: i64,
    pub content: String,
    pub content_hash: String,
    pub token_count: i64,
    pub enabled: bool,
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
pub struct AiProviderEndpoint {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub base_url: String,
    pub webhook_url: String,
    pub api_backend: String,
    pub model: String,
    pub reasoning_effort: String,
    #[serde(skip_serializing)]
    pub secret_ref: String,
    pub priority: i64,
    pub enabled: bool,
    pub api_key_configured: bool,
    pub health_status: String,
    pub failure_count: i64,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_error: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
pub struct Activity {
    pub id: i64,
    pub account_id: String,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub ai_optimize: bool,
    pub ai_instructions: String,
    pub timezone: String,
    pub start_date: String,
    pub end_date: String,
    pub weekdays: Vec<u8>,
    pub send_times: Vec<String>,
    pub group_ids: Vec<i64>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub source_key: String,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRun {
    pub id: i64,
    pub activity_id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub scheduled_for: DateTime<Utc>,
    pub run_key: String,
    pub state: String,
    pub text: String,
    pub content_source: String,
    pub attempts: i64,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub last_error: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DueActivityRun {
    pub run: ActivityRun,
    pub activity: Activity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPreview {
    pub text: String,
    pub source: String,
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
pub struct UniqueRun {
    pub id: i64,
    pub account_id: String,
    pub group_id: i64,
    pub run_key: String,
    pub state: String,
    pub attempts: i64,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub last_error: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
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
    pub receipt_json: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAppRecord {
    pub account_id: String,
    pub app_id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub status: String,
    pub status_detail: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAppRun {
    pub id: i64,
    pub account_id: String,
    pub app_id: String,
    pub group_id: i64,
    pub message_id: i64,
    pub run_key: String,
    pub status: String,
    pub freshness: String,
    pub ai_used: bool,
    pub reply: String,
    pub error: String,
    pub elapsed_ms: i64,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

fn default_json_array() -> String {
    "[]".into()
}

fn default_true() -> bool {
    true
}

fn default_machine_rule_type() -> String {
    "machine".into()
}

fn default_global_scope() -> String {
    "global".into()
}

fn default_medium_priority() -> String {
    "medium".into()
}
