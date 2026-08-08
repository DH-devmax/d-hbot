#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activities;
mod ai;
mod bridge;
mod build_channel;
mod business_apps;
#[cfg(feature = "fixture")]
mod calibration;
mod cardnames;
mod commands;
mod contracts;
mod database;
mod defaults;
mod diagnostics;
pub mod error;
#[cfg(any(feature = "fixture", test))]
pub mod fixture;
pub mod gateway;
mod http_body;
mod knowledge;
pub mod models;
mod moderation;
mod paths;
mod platform;
mod prediction;
mod queue_kernel;
mod repository;
mod runtime;
mod runtime_work;
mod scheduler;
mod secrets;
mod shutdown;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::menu::MenuBuilder;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use build_channel::BuildChannel;
use business_apps::{
    BusinessAppContext, BusinessAppHealth, BusinessAppRegistry, PREDICTION_APP_ID,
};
#[cfg(feature = "fixture")]
use calibration::{CalibrationMetadata, DeveloperCalibrationStatus};
use database::{Database, DatabaseExecutor, DatabaseStatus};
use diagnostics::{redact, SupportBundleResult};
use error::{AppError, AppResult};
#[cfg(feature = "fixture")]
use gateway::ConnectionStatus;
use gateway::{CdpClient, CdpGateway, DiagnosticSnapshot, GatewayReceipt, RuntimeGateway};
use models::{
    ActionRecord, Activity, ActivityPreview, ActivityRun, AiProviderEndpoint, AuditEvent,
    BusinessAppRecord, BusinessAppRun, CardPlan, CardPreview, CardRenameJob, DailySummary, Group,
    GroupAiPermissions, GroupAnnouncement, GroupMuteState, GroupSchedule, KnowledgeBase,
    KnowledgeBinding, KnowledgeChunk as StoredKnowledgeChunk, KnowledgeDocument, Member, MemberRef,
    MemberRoster, Message, ModerationRule, Page, ScheduleRun, TaskItem,
};
use paths::AppPaths;
use runtime_work::{RuntimeCoordination, RuntimeWorkSnapshot};
use secrets::SecretStore;
use shutdown::ShutdownSignal;

pub use ai::{
    AiConfig as RuntimeAiConfig, AiProvider as RuntimeAiProvider, AiRequest as RuntimeAiRequest,
};
#[cfg(feature = "fixture")]
pub use database::{Database as FixtureDatabase, DatabaseExecutor as FixtureDatabaseExecutor};
#[cfg(feature = "fixture")]
pub use diagnostics::Logger as FixtureLogger;
pub use moderation::{DeterministicSemanticClassifier, SemanticClassifier};
#[cfg(feature = "fixture")]
pub use paths::AppPaths as FixtureAppPaths;
#[cfg(feature = "fixture")]
pub use prediction::FixturePredictionSource;
pub use prediction::{PredictionSource, PredictionSourceResult};
pub use runtime::{
    AiProviderFactory, BackendRuntime, Clock, ConfiguredAiProviderFactory, NoopEventSink,
    RuntimeDependencies, RuntimeEventSink, SystemClock,
};
#[cfg(feature = "fixture")]
pub use secrets::SecretStore as FixtureSecretStore;
#[cfg(feature = "fixture")]
pub use shutdown::ShutdownSignal as FixtureShutdownSignal;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub name: &'static str,
    pub version: &'static str,
    pub data_dir: String,
    pub clean_database: bool,
    pub runtime_mode: &'static str,
    pub build_channel: &'static str,
    pub fixture_available: bool,
}

pub(crate) fn default_reasoning_effort() -> String {
    "low".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageQuery {
    account_id: String,
    #[serde(default)]
    group_ids: Vec<i64>,
    keyword: Option<String>,
    kind: Option<String>,
    processing_state: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuditQuery {
    account_id: String,
    group_id: Option<i64>,
    user_id: Option<i64>,
    event: Option<String>,
    level: Option<String>,
    from: Option<chrono::DateTime<Utc>>,
    to: Option<chrono::DateTime<Utc>>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum GroupBatchAction {
    Announcement,
    Mute,
    Unmute,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupBatchInput {
    action: GroupBatchAction,
    group_ids: Vec<i64>,
    text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WangStartupEvent {
    event_id: String,
    status: String,
    detail: String,
    needs_confirmation: bool,
}

pub(crate) fn wang_startup_event(
    status: impl Into<String>,
    detail: impl Into<String>,
    needs_confirmation: bool,
) -> WangStartupEvent {
    WangStartupEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        status: status.into(),
        detail: detail.into(),
        needs_confirmation,
    }
}

pub(crate) fn publish_wang_startup_status(
    app: &tauri::AppHandle,
    pending: Option<&Arc<Mutex<Option<WangStartupEvent>>>>,
    payload: WangStartupEvent,
) {
    if let Some(pending) = pending {
        if let Ok(mut value) = pending.lock() {
            *value = Some(payload.clone());
        }
    }
    let _ = app.emit("wangshangliao-status", payload);
}

pub(crate) async fn persist_wang_installation(
    database: &DatabaseExecutor,
    candidate: &platform::InstallCandidate,
) -> AppResult<()> {
    for (key, value) in [
        ("wangshangliao.path", candidate.path.clone()),
        ("wangshangliao.path_source", candidate.source.clone()),
        ("wangshangliao.path_version", candidate.version.clone()),
        (
            "wangshangliao.path_last_verified_at",
            chrono::Utc::now().to_rfc3339(),
        ),
    ] {
        database.set_setting(key.into(), value, false).await?;
    }
    Ok(())
}

pub(crate) async fn clear_wang_installation(database: &DatabaseExecutor) {
    for key in [
        "wangshangliao.path",
        "wangshangliao.path_source",
        "wangshangliao.path_version",
        "wangshangliao.path_last_verified_at",
    ] {
        let _ = database.set_setting(key.into(), String::new(), false).await;
    }
}

pub(crate) async fn resolve_and_persist_wang_installation(
    app: &tauri::AppHandle,
    database: &DatabaseExecutor,
    requested_path: Option<String>,
    pending: Option<&Arc<Mutex<Option<WangStartupEvent>>>>,
) -> AppResult<platform::InstallCandidate> {
    let progress_app = app.clone();
    let progress_pending = pending.cloned();
    let progress: platform::DiscoveryProgress = Arc::new(move |status, detail| {
        publish_wang_startup_status(
            &progress_app,
            progress_pending.as_ref(),
            wang_startup_event(status, detail, false),
        );
    });
    match platform::resolve_installation(requested_path, Some(progress)).await {
        Ok(candidate) => {
            persist_wang_installation(database, &candidate).await?;
            Ok(candidate)
        }
        Err(error) => {
            clear_wang_installation(database).await;
            Err(error)
        }
    }
}

pub(crate) async fn reconcile_wang_process_path(
    database: &DatabaseExecutor,
    result: &mut platform::WangStartResult,
) {
    let Some(process_path) = result
        .process
        .as_ref()
        .map(|process| process.image_path.clone())
    else {
        return;
    };
    match platform::resolve_installation(Some(process_path), None).await {
        Ok(candidate) => {
            if let Err(error) = persist_wang_installation(database, &candidate).await {
                result.detail.push_str(&format!(
                    "本地旺商聊路径记录更新失败，但外部进程已成功：{}。",
                    error.message
                ));
            }
        }
        Err(error) => result.detail.push_str(&format!(
            "本地旺商聊路径记录更新失败，但外部进程已成功：{}。",
            error.message
        )),
    }
}

pub struct AppState {
    pub paths: AppPaths,
    pub database_executor: DatabaseExecutor,
    pub secrets: SecretStore,
    pub gateway: Arc<dyn RuntimeGateway>,
    pub shutdown: Arc<ShutdownSignal>,
    pub runtime_tasks: Arc<Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>>,
    pub exit_started: Arc<AtomicBool>,
    pub tray_notice_shown: Arc<AtomicBool>,
    pub close_behavior: Arc<RwLock<String>>,
    pub ai_pool: ai::AiProviderPool,
    pub prediction_source: Arc<dyn PredictionSource>,
    pub runtime_coordination: RuntimeCoordination,
    startup_status: Arc<Mutex<Option<WangStartupEvent>>>,
    wang_start_lock: Arc<tokio::sync::Mutex<()>>,
    pub logger: diagnostics::Logger,
}

impl AppState {
    fn initialize() -> AppResult<Self> {
        let paths = AppPaths::discover()?;
        let _ = paths.prepare()?;
        diagnostics::install_panic_hook(paths.logs.clone());
        let logger = diagnostics::Logger::new(paths.logs.clone());
        let database = Database::open(&paths).map_err(|error| {
            let wrapped = AppError::new(
                error.code,
                format!(
                    "{}\n数据文件：{}\n原文件保持不变，请从迁移快照恢复或将日志交给维护人员。",
                    error.message,
                    paths.database.display()
                ),
            );
            logger.write("ERROR", &wrapped.message);
            wrapped
        })?;
        let database_executor = DatabaseExecutor::start(database)?;
        // The endpoint is a build-time/runtime-mode decision.  In particular,
        // the public binary never trusts stale database settings or caller URLs.
        let devtools_url = paths.default_devtools_url().to_string();
        let gateway: Arc<dyn RuntimeGateway> =
            Arc::new(CdpGateway::new(CdpClient::new(devtools_url)?));
        let prediction_source: Arc<dyn PredictionSource> = Arc::new(
            prediction::PublicLotterySource::new(Duration::from_secs(8))?,
        );
        Ok(Self {
            secrets: SecretStore::new(paths.secrets.clone()),
            paths,
            database_executor,
            gateway,
            shutdown: Arc::new(ShutdownSignal::default()),
            runtime_tasks: Arc::new(Mutex::new(Vec::new())),
            exit_started: Arc::new(AtomicBool::new(false)),
            tray_notice_shown: Arc::new(AtomicBool::new(false)),
            close_behavior: Arc::new(RwLock::new("ask".into())),
            ai_pool: ai::AiProviderPool::default(),
            prediction_source,
            runtime_coordination: RuntimeCoordination::default(),
            startup_status: Arc::new(Mutex::new(None)),
            wang_start_lock: Arc::new(tokio::sync::Mutex::new(())),
            logger,
        })
    }
}

pub(crate) fn normalize_close_behavior(value: Option<&str>) -> &'static str {
    match value {
        Some("tray") => "tray",
        Some("exit") => "exit",
        _ => "ask",
    }
}

pub(crate) fn cached_close_behavior(state: &AppState) -> String {
    state
        .close_behavior
        .read()
        .map(|value| normalize_close_behavior(Some(value.as_str())).to_string())
        .unwrap_or_else(|_| "ask".into())
}

pub(crate) fn wang_auto_start_enabled(value: Option<&str>) -> bool {
    value != Some("false")
}

pub(crate) async fn ai_endpoint_configs(
    state: &AppState,
    account_id: &str,
    endpoint_id: Option<i64>,
) -> AppResult<Vec<(AiProviderEndpoint, ai::AiConfig)>> {
    state
        .database_executor
        .ensure_ai_provider_endpoints(account_id.to_string())
        .await?;
    let secrets = state.secrets.load()?;
    let endpoints = state
        .database_executor
        .list_ai_provider_endpoints(account_id.to_string())
        .await?;
    Ok(endpoints
        .into_iter()
        .filter(|endpoint| endpoint.enabled && endpoint_id.is_none_or(|id| id == endpoint.id))
        .map(|endpoint| {
            let config = ai::AiConfig {
                base_url: endpoint.base_url.clone(),
                webhook_url: endpoint.webhook_url.clone(),
                api_backend: endpoint.api_backend.clone(),
                model: endpoint.model.clone(),
                reasoning_effort: endpoint.reasoning_effort.clone(),
                api_key: secrets
                    .get(&endpoint.secret_ref)
                    .cloned()
                    .unwrap_or_default(),
                timeout: ai::AI_PROVIDER_TIMEOUT,
            };
            (endpoint, config)
        })
        .collect())
}

pub(crate) fn validate_group_batch_input(
    input: GroupBatchInput,
) -> AppResult<(GroupBatchAction, Vec<i64>, String)> {
    let mut group_ids = Vec::with_capacity(input.group_ids.len());
    for group_id in input.group_ids {
        if group_id <= 0 {
            return Err(AppError::new("invalid_group_id", "群 ID 必须为正数"));
        }
        if !group_ids.contains(&group_id) {
            group_ids.push(group_id);
        }
    }
    if group_ids.is_empty() {
        return Err(AppError::new("groups_empty", "请至少选择一个群"));
    }

    let text = input.text.unwrap_or_default().trim().to_string();
    if matches!(input.action, GroupBatchAction::Announcement) {
        if text.is_empty() {
            return Err(AppError::new("announcement_empty", "群公告内容不能为空"));
        }
        if text.chars().count() > 1000 {
            return Err(AppError::new(
                "announcement_too_long",
                "群公告最多 1000 个字符",
            ));
        }
    }
    Ok((input.action, group_ids, text))
}

pub(crate) fn normalize_and_validate_rule(rule: &mut ModerationRule) -> AppResult<()> {
    if rule.mode == "auto" {
        rule.mode = "automatic".into();
    }
    if !matches!(rule.rule_type.as_str(), "machine" | "ai")
        || !matches!(rule.scope.as_str(), "global" | "selected")
        || !matches!(rule.priority_level.as_str(), "low" | "medium" | "high")
    {
        return Err(AppError::new(
            "rule_validation",
            "规则类型、范围或优先级无效",
        ));
    }
    if !matches!(rule.mode.as_str(), "observe" | "automatic") {
        return Err(AppError::new(
            "rule_validation",
            "规则执行模式必须为观察或自动",
        ));
    }
    let valid_matcher = match rule.rule_type.as_str() {
        "ai" => rule.matcher == "semantic",
        _ => matches!(
            rule.matcher.as_str(),
            "contains"
                | "exact"
                | "prefix"
                | "regex"
                | "length"
                | "lines"
                | "image_count"
                | "blacklist"
                | "rename_count"
        ),
    };
    if !valid_matcher {
        return Err(AppError::new(
            "rule_matcher",
            "机器规则与 AI 控制规则的匹配方式不能混用",
        ));
    }
    rule.name = rule.name.trim().to_string();
    if rule.name.is_empty() || rule.name.chars().count() > 100 {
        return Err(AppError::new(
            "rule_name",
            "规则名称不能为空且不能超过 100 个字符",
        ));
    }
    if matches!(
        rule.matcher.as_str(),
        "contains" | "exact" | "prefix" | "regex" | "semantic"
    ) && rule.pattern.trim().is_empty()
    {
        return Err(AppError::new("rule_pattern", "规则匹配内容不能为空"));
    }
    if rule.matcher == "regex" {
        regex::Regex::new(&rule.pattern)
            .map_err(|error| AppError::new("rule_regex", format!("正则表达式格式有误：{error}")))?;
    }
    if rule.rule_type == "ai" && !(0.0..=1.0).contains(&rule.semantic_threshold) {
        return Err(AppError::new(
            "rule_threshold",
            "AI 置信度必须在 0 到 1 之间",
        ));
    }
    let mut seen_groups = std::collections::HashSet::new();
    rule.group_ids
        .retain(|group_id| *group_id > 0 && seen_groups.insert(*group_id));
    if rule.scope == "global" {
        rule.group_ids.clear();
    } else if rule.group_ids.is_empty() {
        return Err(AppError::new("rule_groups", "指定群规则至少选择一个群"));
    }
    let mut seen_members = std::collections::HashSet::new();
    rule.whitelist_user_ids
        .retain(|user_id| *user_id > 0 && seen_members.insert(*user_id));
    for action in &rule.actions {
        if !matches!(
            action.kind.as_str(),
            "recall" | "mute" | "remove" | "blacklist" | "notify" | "reply"
        ) {
            return Err(AppError::new("rule_action", "规则包含不支持的动作"));
        }
        if action.kind == "mute" && action.duration_seconds <= 0 {
            return Err(AppError::new("rule_action", "禁言时长必须大于 0 秒"));
        }
    }
    rule.cooldown_seconds = 0;
    rule.exempt_roles.clear();
    rule.exempt_user_ids = rule.whitelist_user_ids.clone();
    Ok(())
}

pub(crate) fn audit_event_label(value: &str) -> &str {
    match value {
        "message_received" => "收到群消息",
        "message_processed" => "群消息处理完成",
        "message_ignored" => "群消息未进入处理",
        "member_event_fallback" => "成员事件转为名单对账",
        "rule_matched" => "规则命中",
        "machine_rule_evaluated" => "机器规则检测",
        "ai_rule_evaluated" => "AI 规则检测",
        "ai_rule_failed" => "AI 规则检测失败",
        "action_executed" => "执行群管动作",
        "effect_dispatched" => "协议动作执行结果",
        "ai_reply" => "AI 回复",
        "ai_reply_failed" => "AI 回复失败",
        "member_joined" => "成员入群",
        "member_left" => "成员离群",
        "member_updated" => "成员资料变更",
        "card_renamed" => "群名片修改",
        "card_rename_job" => "群名片任务执行结果",
        "locked_card_restore_queued" => "锁定名片恢复排队",
        "blacklisted_member_rejoined" => "黑名单成员重新入群",
        "schedule_open" | "schedule_open_group" => "定时开群",
        "schedule_close" | "schedule_close_group" => "定时关群",
        "daily_summary" => "生成每日摘要",
        "daily_summary_failed" => "每日摘要失败",
        "task_reminder_queued" => "任务提醒排队",
        "activity_queued" => "活动进入发布队列",
        "activity_published" => "活动发布成功",
        "activity_publish_failed" => "活动发布失败",
        "semantic_classifier_fallback" => "语义分类降级",
        "gateway_queue_overflow" => "消息队列溢出",
        "automation_paused" => "自动化已暂停",
        "人工更新群公告" => "人工发布新群公告",
        _ => value,
    }
}

pub(crate) fn localize_audit_details(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return localize_plain_audit_details(raw);
    };
    render_localized_audit_value(&value, false)
}

pub(crate) fn localize_plain_audit_details(raw: &str) -> String {
    let localized = raw
        .replace("消息类型=text", "消息类型=文本")
        .replace("消息类型=image", "消息类型=图片")
        .replace("消息类型=card", "消息类型=名片")
        .replace("消息类型=notice", "消息类型=通知")
        .replace("消息类型=other", "消息类型=其他")
        .replace("收到 NIM 入群事件", "收到旺商聊成员入群事件")
        .replace("收到 NIM 离群事件", "收到旺商聊成员离群事件")
        .replace("收到 NIM 成员资料更新事件", "收到旺商聊成员资料更新事件");
    if localized.contains("消息类型=") && localized.contains("，序号=") {
        format!(
            "{}（旧版记录仅保存消息类型和队列序号）",
            localized.replace("，序号=", "，接收队列序号=")
        )
    } else {
        localized
    }
}

pub(crate) fn render_localized_audit_value(value: &serde_json::Value, nested: bool) -> String {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| render_localized_audit_value(value, true))
            .collect::<Vec<_>>()
            .join("、"),
        serde_json::Value::Object(values) => values
            .iter()
            .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
            .map(|(key, value)| {
                format!(
                    "{}：{}",
                    audit_detail_key_label(key),
                    render_localized_audit_value(value, true)
                )
            })
            .collect::<Vec<_>>()
            .join(if nested { "；" } else { "　" }),
        serde_json::Value::String(value) => audit_detail_value_label(value).to_string(),
        serde_json::Value::Bool(value) => if *value { "是" } else { "否" }.to_string(),
        serde_json::Value::Null => String::new(),
        value => value.to_string(),
    }
}

pub(crate) fn audit_detail_key_label(value: &str) -> &str {
    match value {
        "status" => "状态",
        "success" => "是否成功",
        "effect" => "自动动作",
        "error" => "错误",
        "receipt" => "协议回执",
        "route" => "协议路由",
        "transportCode" => "传输状态码",
        "transportErrno" => "传输错误码",
        "businessCode" => "业务状态码",
        "businessErrno" => "业务错误码",
        "businessMessage" => "业务说明",
        "requestId" => "请求编号",
        "messageId" => "本地消息编号",
        "session" => "会话",
        "verification" => "回读验证",
        "acknowledged" => "本次确认数量",
        "acknowledgedThrough" => "已确认至序号",
        "remaining" => "剩余数量",
        "dropped" => "丢弃数量",
        "retryable" => "可重试",
        "unknown" => "结果待确认",
        "groupId" => "群",
        "userId" => "成员",
        "durationSeconds" => "时长（秒）",
        "action" => "操作",
        "muted" => "全员禁言",
        "noticeId" => "公告编号",
        "content" => "内容",
        "actionKind" => "动作",
        "ruleId" => "规则编号",
        "matchedRuleIds" => "命中规则",
        "contributorRuleIds" => "动作来源规则",
        "automatic" => "自动执行",
        "elapsedMs" => "耗时（毫秒）",
        "scores" => "语义置信度",
        "confidence" => "置信度",
        "decision" => "执行决定",
        "mode" => "执行模式",
        "reason" => "原因",
        _ => value,
    }
}

pub(crate) fn audit_detail_value_label(value: &str) -> &str {
    match value {
        "incoming" => "收到",
        "outgoing" => "发出",
        "text" => "文本",
        "image" => "图片",
        "card" => "名片",
        "notice" => "通知",
        "other" => "其他",
        "pending" => "待处理",
        "queued" => "已入库并排队",
        "processing" => "处理中",
        "processed" => "已处理",
        "ignored" => "已忽略",
        "rules-and-ai-evaluated" => "规则与 AI 检查完成",
        "persisted-and-acknowledged" => "已保存并确认源队列",
        "roster-reconciliation" => "由 60 秒成员名单对账补齐",
        "succeeded" => "成功",
        "failed" => "失败",
        "unknown" => "待人工确认",
        "verified" => "已回读确认",
        "unsupported" => "未开放",
        "not-applicable" => "无需回读",
        "group_mute" => "全群发言控制",
        "automatic" => "自动执行",
        "observe" => "观察",
        "recall" => "撤回",
        "mute" => "禁言",
        "unmute" => "解除禁言",
        "remove" => "移出成员",
        "blacklist" => "加入黑名单",
        "notify" => "提示",
        "send_text" => "发送文字",
        "OK" => "正常",
        "true" => "是",
        "false" => "否",
        _ => value,
    }
}

pub(crate) fn redact_archived_receipt(mut receipt: GatewayReceipt) -> GatewayReceipt {
    receipt.route = redact(&receipt.route);
    receipt.status = redact(&receipt.status);
    receipt.business_message = redact(&receipt.business_message);
    receipt.request_id = redact(&receipt.request_id);
    receipt.message_id = redact(&receipt.message_id);
    receipt.session = redact(&receipt.session);
    receipt
}

pub(crate) fn manual_event_name(kind: &str) -> &'static str {
    match kind {
        "send_text" => "人工发送群消息",
        "recall" => "人工撤回消息",
        "mute" => "人工禁言成员",
        "unmute" => "人工解禁成员",
        "rename" => "人工修改群名片",
        "remove" => "人工移出成员",
        "blacklist" => "人工加入黑名单",
        "unblacklist" => "人工移出黑名单",
        "group_mute" => "人工设置全群发言",
        "announcement" => "人工发布新群公告",
        "announcement_update" => "人工编辑群公告",
        "announcement_delete" => "人工删除群公告",
        _ => "人工群管操作",
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn archive_manual_gateway_result(
    state: &State<'_, AppState>,
    account_id: &str,
    group_id: i64,
    user_id: i64,
    kind: &str,
    duration_seconds: i64,
    reason: &str,
    result: AppResult<GatewayReceipt>,
) -> AppResult<GatewayReceipt> {
    let (success, unknown, error_text, receipt) = match &result {
        Ok(receipt) => (true, false, String::new(), receipt.clone()),
        Err(error) => {
            let unknown = error.delivery_outcome_unknown();
            let mut receipt = GatewayReceipt::failed(kind, error);
            if unknown {
                receipt.status = "unknown".into();
            }
            (false, unknown, error.message.clone(), receipt)
        }
    };
    let error_text = redact(&error_text);
    let receipt = redact_archived_receipt(receipt);
    let receipt_json = serde_json::to_string(&receipt).unwrap_or_else(|_| "{}".into());
    let action_result = state
        .database_executor
        .record_action(ActionRecord {
            id: 0,
            account_id: account_id.into(),
            group_id,
            user_id,
            message_id: None,
            rule_id: None,
            kind: kind.into(),
            mode: "manual".into(),
            duration_seconds,
            reason: reason.into(),
            success,
            error: error_text.clone(),
            receipt_json: receipt_json.clone(),
            dedupe_key: format!("manual:{}", uuid::Uuid::new_v4()),
            created_at: Utc::now(),
        })
        .await;
    let audit_result = state
        .database_executor
        .record_audit(AuditEvent {
            id: 0,
            account_id: account_id.into(),
            group_id,
            user_id,
            actor: "当前管理员".into(),
            event: manual_event_name(kind).into(),
            level: if success {
                "info"
            } else if unknown {
                "warning"
            } else {
                "error"
            }
            .into(),
            details: serde_json::json!({
                "status": if success { "succeeded" } else if unknown { "unknown" } else { "failed" },
                "error": error_text,
                "receipt": receipt,
            })
            .to_string(),
            created_at: Utc::now(),
        })
        .await;
    if let Err(error) = action_result.and(audit_result.map(|_| 0)) {
        state
            .logger
            .write("ERROR", &format!("人工群管操作归档失败：{}", error.message));
        if result.is_ok() {
            return Err(AppError::new(
                "manual_action_archive",
                "操作已发送，但本地归档失败，请查看调试日志",
            ));
        }
    }
    result
}

pub(crate) async fn require_manager(state: &State<'_, AppState>, group_id: i64) -> AppResult<()> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    }) {
        Ok(())
    } else {
        Err(AppError::new(
            "management_required",
            "需要将账号权限设置为管理",
        ))
    }
}

pub(crate) async fn require_member_manager(
    state: &State<'_, AppState>,
    group_id: i64,
) -> AppResult<MemberRoster> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    }) {
        Ok(roster)
    } else {
        Err(AppError::new(
            "management_required",
            "需要将账号权限设置为管理员",
        ))
    }
}

pub(crate) fn canonical_member(roster: &MemberRoster, user_id: i64) -> AppResult<Member> {
    let member = roster
        .members
        .iter()
        .find(|member| member.user_id == user_id && member.user_id > 0 && member.present)
        .cloned()
        .ok_or_else(|| AppError::new("member_identity", "目标成员不在当前群名单中"))?;
    if roster.synthetic_user_ids.contains(&member.user_id) {
        return Err(AppError::new(
            "member_identity_synthetic",
            "目标成员只有临时身份，不能执行群成员写操作",
        ));
    }
    if matches!(member.role.as_str(), "owner" | "admin") {
        return Err(AppError::new(
            "member_role_protected",
            "不能对群主或管理员执行成员写操作",
        ));
    }
    Ok(member)
}

pub(crate) fn canonical_member_ref(
    roster: &MemberRoster,
    requested: &MemberRef,
) -> AppResult<Member> {
    let requested_user_id = requested.user_id.filter(|value| *value > 0);
    let requested_nim_id = requested
        .nim_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    if requested_user_id.is_none() && requested_nim_id.is_none() {
        return Err(AppError::new("member_identity", "目标成员缺少有效身份"));
    }
    let matches = roster
        .members
        .iter()
        .filter(|member| {
            let user_matches = requested_user_id.is_none_or(|user_id| member.user_id == user_id);
            let nim_matches = requested_nim_id.is_none_or(|nim_id| member.nim_id == nim_id);
            user_matches && nim_matches
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(AppError::new(
            "member_identity",
            "目标成员身份无法唯一映射到当前名单",
        ));
    }
    canonical_member(roster, matches[0].user_id)
}

pub(crate) async fn require_account(
    state: &State<'_, AppState>,
    account_id: &str,
) -> AppResult<i64> {
    let (sender_id, current_account) = state.gateway.session_identity().await?;
    if current_account == account_id {
        Ok(sender_id)
    } else {
        Err(AppError::new(
            "account_mismatch",
            "当前登录账号与操作数据不一致，请刷新后重试",
        ))
    }
}

macro_rules! dh_handlers {
    ($($developer:path),* $(,)?) => {
        tauri::generate_handler![
            commands::system::health,
            commands::system::get_runtime_work_snapshot,
            commands::system::acknowledge_runtime_work_failures,
            commands::system::database_status,
            commands::system::export_support_bundle,
            commands::system::get_close_behavior,
            commands::system::reset_close_behavior,
            commands::system::resolve_close_action,
            commands::system::diagnose,
            commands::groups::list_groups,
            commands::groups::list_cached_groups,
            commands::groups::list_members,
            commands::groups::local_members,
            commands::ai::get_ai_settings,
            commands::ai::get_ai_automation_settings,
            commands::ai::save_ai_automation_settings,
            commands::cards::get_card_settings,
            commands::cards::save_card_settings,
            commands::cards::save_group_welcome,
            commands::ai::save_ai_settings,
            commands::ai::list_ai_provider_endpoints,
            commands::ai::save_ai_provider_endpoint,
            commands::ai::delete_ai_provider_endpoint,
            commands::ai::test_ai_provider_endpoint,
            commands::messaging::send_text,
            commands::messaging::send_text_batch,
            commands::messaging::execute_group_batch,
            commands::messaging::query_messages,
            commands::messaging::recent_messages,
            commands::groups::set_group_features,
            commands::rules::list_rules,
            commands::rules::set_group_rule_features,
            commands::rules::search_rule_members,
            commands::rules::save_rule,
            commands::rules::delete_rule,
            commands::rules::export_rules,
            commands::rules::import_rules,
            commands::knowledge::list_knowledge_bases,
            commands::knowledge::create_knowledge_base,
            commands::knowledge::update_knowledge_base,
            commands::knowledge::clone_knowledge_base,
            commands::knowledge::delete_knowledge_base,
            commands::knowledge::list_knowledge_documents,
            commands::knowledge::save_knowledge_document,
            commands::knowledge::delete_knowledge_document,
            commands::knowledge::bind_knowledge_base,
            commands::knowledge::list_knowledge_bindings,
            commands::tasks::list_tasks,
            commands::tasks::save_task,
            commands::tasks::delete_task,
            commands::tasks::list_activities,
            commands::tasks::save_activity,
            commands::tasks::delete_activity,
            commands::tasks::list_activity_runs,
            commands::tasks::preview_activity_text,
            commands::tasks::publish_activity_now,
            commands::tasks::list_schedules,
            commands::tasks::save_schedule,
            commands::tasks::delete_schedule,
            commands::tasks::list_schedule_runs,
            commands::audit::list_audit,
            commands::audit::query_audit,
            commands::audit::export_audit,
            commands::summaries::list_daily_summaries,
            commands::summaries::get_summary_settings,
            commands::summaries::save_summary_settings,
            commands::summaries::generate_daily_summary,
            commands::ai::test_ai,
            commands::bizapps::list_business_apps,
            commands::bizapps::set_business_app_enabled,
            commands::bizapps::get_business_app_health,
            commands::bizapps::list_business_app_runs,
            commands::bizapps::test_business_app,
            commands::wang::locate_wangshangliao,
            commands::wang::get_wang_startup_settings,
            commands::wang::save_wang_startup_settings,
            commands::wang::take_wang_startup_status,
            commands::wang::start_wangshangliao,
            commands::wang::focus_wangshangliao,
            commands::wang::inspect_wangshangliao,
            commands::wang::get_gateway_capabilities,
            commands::wang::get_wang_maintenance_result,
            commands::wang::get_wang_profile_status,
            commands::wang::apply_wang_profile_patch,
            commands::wang::restore_wang_profile_patch,
            commands::moderation::recall_message,
            commands::moderation::mute_member,
            commands::moderation::unmute_member,
            commands::moderation::rename_member,
            commands::moderation::remove_member,
            commands::moderation::set_group_mute,
            commands::moderation::get_group_mute_state,
            commands::moderation::set_group_announcement,
            commands::moderation::get_group_announcement,
            commands::moderation::list_group_announcements,
            commands::moderation::update_group_announcement,
            commands::moderation::delete_group_announcement,
            commands::moderation::get_group_management_context,
            commands::moderation::execute_member_batch,
            commands::cards::preview_card_names,
            commands::cards::apply_card_names,
            commands::cards::list_card_rename_jobs,
            commands::cards::retry_card_rename_jobs,
            $($developer),*
        ]
    };
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn set_tray_menu(app: &tauri::AppHandle, paused: bool) -> tauri::Result<()> {
    let pause_label = if paused {
        "恢复全部自动化"
    } else {
        "暂停全部自动化"
    };
    let tray_menu = MenuBuilder::new(app)
        .text("show", "显示 DH BOT")
        .text("pause", pause_label)
        .separator()
        .text("exit", "退出 DH BOT")
        .build()?;
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(tray_menu))?;
    }
    Ok(())
}

pub(crate) fn show_tray_notification(app: &tauri::AppHandle) {
    let should_show = app
        .try_state::<AppState>()
        .map(|state| !state.tray_notice_shown.swap(true, Ordering::AcqRel))
        .unwrap_or(false);
    if should_show {
        let _ = app
            .notification()
            .builder()
            .title("DH BOT 已在后台运行")
            .body("可从 Windows 右下角托盘恢复窗口。")
            .show();
    }
}

pub(crate) fn request_graceful_exit(app: tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        app.exit(0);
        return;
    };
    if state.exit_started.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    state.shutdown.cancel();
    let database = state.database_executor.clone();
    let tasks = state.runtime_tasks.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let worker_deadline = deadline - Duration::from_secs(1);
        let mut handles = tasks
            .lock()
            .map(|mut handles| std::mem::take(&mut *handles))
            .unwrap_or_default();
        let worker_wait = async {
            for handle in &mut handles {
                let _ = handle.await;
            }
        };
        let timed_out = tokio::time::timeout_at(worker_deadline, worker_wait)
            .await
            .is_err();
        if timed_out {
            for handle in &handles {
                handle.abort();
            }
            let unknown_deadline = deadline
                .checked_sub(Duration::from_millis(500))
                .unwrap_or(deadline);
            let _ = tokio::time::timeout_at(
                unknown_deadline,
                database.mark_processing_effects_unknown(
                    "DH BOT 退出等待超时，远端执行结果未确认".into(),
                ),
            )
            .await;
        }
        let _ = tokio::time::timeout_at(deadline, database.shutdown()).await;
        #[cfg(windows)]
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(2));
            std::process::exit(0);
        });
        app.exit(0);
    });
}

async fn capability_calibration_loop(
    app: tauri::AppHandle,
    gateway: Arc<dyn RuntimeGateway>,
    database: DatabaseExecutor,
    shutdown: Arc<ShutdownSignal>,
) {
    let mut calibrated_identity: Option<(String, String)> = None;
    let mut probe_ticks = 60_u8;
    loop {
        let configured_path = database
            .get_setting("wangshangliao.path".into())
            .await
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty());
        let identity = match platform::running_process_identity_for(configured_path).await {
            Ok(Some(process)) => {
                let script_hash = platform::profile_status(Some(process.image_path))
                    .await
                    .map(|status| status.script_hash)
                    .unwrap_or_default();
                Some((process.file_version, script_hash))
            }
            _ => None,
        };
        // Structural capability probing performs several IPC reads. Re-run it
        // immediately when the process identity changes, otherwise every five
        // minutes; message and member workers already monitor connection health.
        if identity != calibrated_identity || probe_ticks >= 60 {
            let mut capabilities = match &identity {
                Some((version, script_hash)) => {
                    gateway.probe_capabilities(version, script_hash).await
                }
                None => gateway.probe_capabilities("", "").await,
            };
            if capabilities.announcement.status == gateway::CapabilityStatus::ManualVerification
                && !capabilities.announcement.fingerprint.is_empty()
                && database
                    .is_gateway_capability_verified(
                        capabilities.announcement.fingerprint.clone(),
                        "announcement".into(),
                    )
                    .await
                    .unwrap_or(false)
            {
                capabilities = gateway.mark_capability_verified("announcement");
            }
            for (name, capability) in [
                ("announcement", &capabilities.announcement),
                ("sendText", &capabilities.send_text),
                ("mute", &capabilities.mute),
                ("recall", &capabilities.recall),
                ("rename", &capabilities.rename),
                ("removeMember", &capabilities.remove_member),
                ("groupMute", &capabilities.group_mute),
                ("memberEvents", &capabilities.member_events),
            ] {
                let source = serde_json::to_value(capability.source)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "wangElectron".into());
                let status = serde_json::to_value(capability.status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unavailable".into());
                let _ = database
                    .save_gateway_capability_verification(
                        capability.fingerprint.clone(),
                        name.into(),
                        source,
                        status,
                        capability.automatic_allowed,
                        capability.fingerprint.clone(),
                        if capability.status == gateway::CapabilityStatus::Unavailable {
                            capability.reason.clone()
                        } else {
                            String::new()
                        },
                    )
                    .await;
            }
            let _ = app.emit("gateway-capabilities", capabilities);
            calibrated_identity = identity;
            probe_ticks = 0;
        } else {
            probe_ticks = probe_ticks.saturating_add(1);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
            _ = shutdown.cancelled() => break,
        }
    }
}

#[cfg(windows)]
async fn auto_start_wangshangliao(
    app: tauri::AppHandle,
    database: DatabaseExecutor,
    shutdown: Arc<ShutdownSignal>,
    startup_status: Arc<Mutex<Option<WangStartupEvent>>>,
    start_lock: Arc<tokio::sync::Mutex<()>>,
) {
    let enabled = wang_auto_start_enabled(
        database
            .get_setting("wangshangliao.auto_start".into())
            .await
            .ok()
            .flatten()
            .as_deref(),
    );
    if !enabled || shutdown.is_cancelled() {
        return;
    }
    let start_guard = start_lock.lock().await;
    let requested_path = database
        .get_setting("wangshangliao.path".into())
        .await
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty());
    let mut path = None;
    let mut start_result = match resolve_and_persist_wang_installation(
        &app,
        &database,
        requested_path,
        Some(&startup_status),
    )
    .await
    {
        Ok(candidate) => {
            path = Some(candidate.path);
            publish_wang_startup_status(
                &app,
                Some(&startup_status),
                wang_startup_event(
                    "starting",
                    "已识别旺商聊，正在启动并连接 DevTools，请稍候。",
                    false,
                ),
            );
            platform::start(path.clone(), "http://127.0.0.1:9222".into(), false).await
        }
        Err(error) => Err(error),
    };
    if let Ok(result) = &start_result {
        if let Some(request_id) = &result.maintenance_request_id {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                match platform::maintenance_result(request_id) {
                    Ok(Some(result)) if result.success => {
                        publish_wang_startup_status(
                            &app,
                            Some(&startup_status),
                            wang_startup_event(
                                "starting",
                                "维护已完成，正在重新启动旺商聊并连接 DevTools，请稍候。",
                                false,
                            ),
                        );
                        start_result =
                            platform::start(path.clone(), "http://127.0.0.1:9222".into(), false)
                                .await;
                        break;
                    }
                    Ok(Some(result)) => {
                        start_result = Err(AppError::new(result.error_code, result.message));
                        break;
                    }
                    Err(error) => {
                        start_result = Err(error);
                        break;
                    }
                    Ok(None) if tokio::time::Instant::now() < deadline => {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {},
                            _ = shutdown.cancelled() => return,
                        }
                    }
                    Ok(None) => {
                        start_result = Err(AppError::new(
                            "maintenance_timeout",
                            "等待旺商聊固定登录分区维护完成超时，请在设置页重新检查",
                        ));
                        break;
                    }
                }
            }
        }
    }
    if let Ok(result) = &mut start_result {
        reconcile_wang_process_path(&database, result).await;
    }
    let payload = match start_result {
        Ok(result) => wang_startup_event(result.status, result.detail, result.needs_confirmation),
        Err(error) => wang_startup_event(error.code, error.message, false),
    };
    publish_wang_startup_status(&app, Some(&startup_status), payload.clone());
    let mut last_problem = if matches!(
        payload.status.as_str(),
        "ready" | "devtools-ready" | "nim-not-ready"
    ) {
        None
    } else {
        Some((payload.status, payload.detail, payload.needs_confirmation))
    };
    drop(start_guard);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3)) => {},
            _ = shutdown.cancelled() => return,
        }
        let enabled = wang_auto_start_enabled(
            database
                .get_setting("wangshangliao.auto_start".into())
                .await
                .ok()
                .flatten()
                .as_deref(),
        );
        if !enabled {
            last_problem = None;
            continue;
        }
        let status = platform::inspect("http://127.0.0.1:9222")
            .await
            .unwrap_or_else(|_| "unavailable".into());
        if matches!(
            status.as_str(),
            "ready" | "devtools-ready" | "nim-not-ready"
        ) {
            if last_problem.take().is_some() {
                let detail = if status == "ready" {
                    "旺商聊协议会话已就绪。"
                } else {
                    "DevTools 已连接，请在旺商聊完成登录，DH BOT 会继续等待会话初始化。"
                };
                publish_wang_startup_status(
                    &app,
                    Some(&startup_status),
                    wang_startup_event(status, detail, false),
                );
            }
            continue;
        }

        let _start_guard = start_lock.lock().await;
        let status = platform::inspect("http://127.0.0.1:9222")
            .await
            .unwrap_or_else(|_| "unavailable".into());
        if matches!(
            status.as_str(),
            "ready" | "devtools-ready" | "nim-not-ready"
        ) {
            last_problem = None;
            continue;
        }
        let requested_path = database
            .get_setting("wangshangliao.path".into())
            .await
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty());
        let payload = match resolve_and_persist_wang_installation(
            &app,
            &database,
            requested_path,
            Some(&startup_status),
        )
        .await
        {
            Ok(candidate) => {
                publish_wang_startup_status(
                    &app,
                    Some(&startup_status),
                    wang_startup_event(
                        "starting",
                        "已识别旺商聊，正在重新连接 DevTools，请稍候。",
                        false,
                    ),
                );
                match platform::start(Some(candidate.path), "http://127.0.0.1:9222".into(), false)
                    .await
                {
                    Ok(mut result) => {
                        reconcile_wang_process_path(&database, &mut result).await;
                        wang_startup_event(result.status, result.detail, result.needs_confirmation)
                    }
                    Err(error) => wang_startup_event(error.code, error.message, false),
                }
            }
            Err(error) => wang_startup_event(error.code, error.message, false),
        };
        let signature = (
            payload.status.clone(),
            payload.detail.clone(),
            payload.needs_confirmation,
        );
        if last_problem.as_ref() == Some(&signature) {
            continue;
        }
        last_problem = Some(signature);
        publish_wang_startup_status(&app, Some(&startup_status), payload);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if platform::run_maintenance_if_requested() {
        return;
    }
    if !platform::ensure_webview2_runtime() {
        return;
    }
    // Register single-instance arbitration before opening SQLite, logs or any
    // background worker. A second launch only restores the existing window.
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app)
        }));
    #[cfg(feature = "fixture")]
    let builder = builder.invoke_handler(dh_handlers![
        commands::developer::get_runtime_mode,
        commands::developer::set_runtime_mode,
        commands::developer::start_fixture_host,
        commands::developer::begin_developer_calibration,
        commands::developer::get_developer_calibration_status,
        commands::developer::cancel_developer_calibration,
        commands::developer::finish_developer_calibration
    ]);
    #[cfg(not(feature = "fixture"))]
    let builder = builder.invoke_handler(dh_handlers![]);
    if let Err(error) = builder
        .on_tray_icon_event(|app, event| {
            let restore = matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } | TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            );
            if restore {
                show_main_window(app);
            }
        })
        .setup(move |app| {
            let state = AppState::initialize()
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            app.manage(state);
            let app_handle = app.handle().clone();
            set_tray_menu(app.handle(), false)?;
            let initial_database = app.state::<AppState>().database_executor.clone();
            let initial_close_behavior = app.state::<AppState>().close_behavior.clone();
            let initial_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let close_behavior = initial_database
                    .get_setting("window.close_behavior".into())
                    .await
                    .ok()
                    .flatten();
                if let Ok(mut current) = initial_close_behavior.write() {
                    *current = normalize_close_behavior(close_behavior.as_deref()).to_string();
                }
                let paused = initial_database
                    .get_setting("automation.mode".into())
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some("paused");
                let _ = set_tray_menu(&initial_app, paused);
                let _ =
                    initial_app.emit("automation-paused", serde_json::json!({"paused": paused}));
            });
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "show" => {
                    show_main_window(app);
                }
                "pause" => {
                    if let Some(state) = app.try_state::<AppState>() {
                        let database = state.database_executor.clone();
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let current = database
                                .get_setting("automation.mode".into())
                                .await
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| "observe".into());
                            let next = if current == "paused" {
                                "observe"
                            } else {
                                "paused"
                            };
                            let _ = database
                                .set_setting("automation.mode".into(), next.into(), false)
                                .await;
                            let _ = set_tray_menu(&app, next == "paused");
                            let _ = app.emit(
                                "automation-paused",
                                serde_json::json!({"paused": next == "paused"}),
                            );
                        });
                    }
                }
                "exit" => {
                    request_graceful_exit(app.clone());
                }
                _ => {}
            });
            let runtime_gateway: Arc<dyn RuntimeGateway> = app.state::<AppState>().gateway.clone();
            let mut runtime_tasks = runtime::BackendRuntime::new_with_ai_pool(
                app.state::<AppState>().database_executor.clone(),
                runtime_gateway,
                app.state::<AppState>().secrets.clone(),
                app.state::<AppState>().shutdown.clone(),
                app.state::<AppState>().logger.clone(),
                app.state::<AppState>().ai_pool.clone(),
                app.state::<AppState>().prediction_source.clone(),
            )
            .with_coordination(app.state::<AppState>().runtime_coordination.clone())
            .spawn(app_handle.clone());
            runtime_tasks.push(bridge::spawn(
                app.state::<AppState>().gateway.clone(),
                app.state::<AppState>().database_executor.clone(),
                app.state::<AppState>().shutdown.clone(),
                app.state::<AppState>().logger.clone(),
            ));
            if app.state::<AppState>().paths.runtime_mode() == "real" {
                runtime_tasks.push(tauri::async_runtime::spawn(capability_calibration_loop(
                    app.handle().clone(),
                    app.state::<AppState>().gateway.clone(),
                    app.state::<AppState>().database_executor.clone(),
                    app.state::<AppState>().shutdown.clone(),
                )));
                #[cfg(windows)]
                runtime_tasks.push(tauri::async_runtime::spawn(auto_start_wangshangliao(
                    app.handle().clone(),
                    app.state::<AppState>().database_executor.clone(),
                    app.state::<AppState>().shutdown.clone(),
                    app.state::<AppState>().startup_status.clone(),
                    app.state::<AppState>().wang_start_lock.clone(),
                )));
            }
            if let Ok(mut tasks) = app.state::<AppState>().runtime_tasks.lock() {
                *tasks = runtime_tasks;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle().clone();
                if app.state::<AppState>().exit_started.load(Ordering::Acquire) {
                    return;
                }
                api.prevent_close();
                let behavior = cached_close_behavior(&app.state::<AppState>());
                app.state::<AppState>()
                    .logger
                    .write("INFO", &format!("收到窗口关闭请求，处理方式：{behavior}"));
                match behavior.as_str() {
                    "tray" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.hide();
                        }
                        show_tray_notification(&app);
                    }
                    "exit" => request_graceful_exit(app),
                    _ => {
                        let _ = app.emit("close-requested", ());
                    }
                }
            }
        })
        .run(tauri::generate_context!())
    {
        platform::show_startup_error(&format!("DH BOT 启动失败：{error}"));
    }
}

#[cfg(test)]
mod close_behavior_tests {
    use super::{
        audit_event_label, localize_audit_details, manual_event_name, normalize_and_validate_rule,
        normalize_close_behavior, redact_archived_receipt, validate_group_batch_input,
        wang_auto_start_enabled, GroupBatchAction, GroupBatchInput,
    };
    use crate::gateway::GatewayReceipt;
    use crate::models::{ModerationRule, RuleAction};

    fn test_rule(rule_type: &str, matcher: &str) -> ModerationRule {
        ModerationRule {
            id: 0,
            account_id: "ACCOUNT".into(),
            rule_type: rule_type.into(),
            scope: "selected".into(),
            group_ids: vec![100, 100, -1],
            priority_level: "medium".into(),
            whitelist_user_ids: vec![200, 200, 0],
            group_id: 100,
            name: " 测试规则 ".into(),
            matcher: matcher.into(),
            pattern: "测试".into(),
            threshold: 0,
            count: 0,
            window_seconds: 0,
            cooldown_seconds: 600,
            priority: 100,
            mode: "auto".into(),
            enabled: false,
            semantic_threshold: 0.8,
            exempt_roles: vec!["owner".into(), "admin".into()],
            exempt_user_ids: vec![200],
            actions: vec![RuleAction {
                kind: "recall".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        }
    }

    #[test]
    fn accepts_only_persisted_close_choices() {
        assert_eq!(normalize_close_behavior(Some("tray")), "tray");
        assert_eq!(normalize_close_behavior(Some("exit")), "exit");
        assert_eq!(normalize_close_behavior(Some("ask")), "ask");
        assert_eq!(normalize_close_behavior(Some("fixture")), "ask");
        assert_eq!(normalize_close_behavior(None), "ask");
    }

    #[test]
    fn wangshangliao_auto_start_defaults_on_and_honors_explicit_off() {
        assert!(wang_auto_start_enabled(None));
        assert!(wang_auto_start_enabled(Some("true")));
        assert!(!wang_auto_start_enabled(Some("false")));
    }

    #[test]
    fn manual_action_archive_uses_chinese_events_and_redacted_receipts() {
        assert_eq!(manual_event_name("recall"), "人工撤回消息");
        let secret = ["sk", "-", "abcdefghijklmnopqrstuvwxyz"].concat();
        let receipt = redact_archived_receipt(GatewayReceipt {
            route: format!("/send?token={secret}"),
            status: "succeeded".into(),
            transport_code: Some(200),
            transport_errno: Some(0),
            business_code: Some(200),
            business_errno: Some(0),
            business_message: format!("Authorization: Bearer {secret}"),
            request_id: secret.clone(),
            message_id: "MESSAGE".into(),
            session: secret.clone(),
            acknowledged_through: 0,
            acknowledged: 0,
            remaining: 0,
            dropped: 0,
            verification: None,
        });
        let archived = serde_json::to_string(&receipt).unwrap();
        assert!(!archived.contains(&secret));
        assert!(archived.contains("***"));
    }

    #[test]
    fn group_batch_validation_deduplicates_in_order_and_limits_announcements() {
        let (action, group_ids, text) = validate_group_batch_input(GroupBatchInput {
            action: GroupBatchAction::Announcement,
            group_ids: vec![20, 10, 20],
            text: Some("  群公告  ".into()),
        })
        .unwrap();
        assert_eq!(action, GroupBatchAction::Announcement);
        assert_eq!(group_ids, vec![20, 10]);
        assert_eq!(text, "群公告");

        assert!(validate_group_batch_input(GroupBatchInput {
            action: GroupBatchAction::Announcement,
            group_ids: vec![1],
            text: Some("超".repeat(1001)),
        })
        .is_err());
        assert!(validate_group_batch_input(GroupBatchInput {
            action: GroupBatchAction::Mute,
            group_ids: vec![0],
            text: None,
        })
        .is_err());
    }

    #[test]
    fn audit_exports_use_chinese_events_fields_and_values() {
        assert_eq!(audit_event_label("effect_dispatched"), "协议动作执行结果");
        let details = localize_audit_details(
            r#"{"effect":"group_mute","success":true,"receipt":{"status":"succeeded","verification":"verified","transportErrno":0}}"#,
        );
        assert!(details.contains("自动动作：全群发言控制"));
        assert!(details.contains("是否成功：是"));
        assert!(details.contains("状态：成功"));
        assert!(details.contains("回读验证：已回读确认"));
        assert!(details.contains("传输错误码：0"));
        assert!(!details.contains("effect"));
        assert!(!details.contains("verification"));
        assert_eq!(
            localize_audit_details("消息类型=other，序号=71"),
            "消息类型=其他，接收队列序号=71（旧版记录仅保存消息类型和队列序号）"
        );
        assert_eq!(
            localize_audit_details("收到 NIM 离群事件"),
            "收到旺商聊成员离群事件"
        );
    }

    #[test]
    fn rule_v2_normalization_removes_legacy_exemptions_and_cooldown() {
        let mut rule = test_rule("machine", "contains");
        normalize_and_validate_rule(&mut rule).unwrap();
        assert_eq!(rule.name, "测试规则");
        assert_eq!(rule.mode, "automatic");
        assert_eq!(rule.group_ids, vec![100]);
        assert_eq!(rule.whitelist_user_ids, vec![200]);
        assert_eq!(rule.exempt_user_ids, vec![200]);
        assert!(rule.exempt_roles.is_empty());
        assert_eq!(rule.cooldown_seconds, 0);
    }

    #[test]
    fn machine_and_ai_matchers_are_strictly_separated() {
        assert!(normalize_and_validate_rule(&mut test_rule("machine", "semantic")).is_err());
        assert!(normalize_and_validate_rule(&mut test_rule("ai", "contains")).is_err());
        assert!(normalize_and_validate_rule(&mut test_rule("ai", "semantic")).is_ok());
    }
}
