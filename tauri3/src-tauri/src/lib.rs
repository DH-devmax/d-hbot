#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod bridge;
mod build_channel;
mod cardnames;
mod contracts;
mod database;
mod diagnostics;
pub mod error;
#[cfg(any(feature = "fixture", test))]
pub mod fixture;
pub mod gateway;
mod knowledge;
pub mod models;
mod moderation;
mod paths;
mod platform;
mod prediction;
mod repository;
mod runtime;
mod scheduler;
mod secrets;

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::menu::MenuBuilder;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{Emitter, Manager, State, WindowEvent};
use tokio::sync::Notify;

use build_channel::BuildChannel;
use database::{Database, DatabaseStatus};
use error::{AppError, AppResult};
use gateway::{CdpClient, CdpGateway, DiagnosticSnapshot, RuntimeGateway};
use models::{
    AuditEvent, CardPlan, CardPreview, CardRenameJob, DailySummary, Group, GroupAiPermissions,
    GroupSchedule, KnowledgeBase, KnowledgeBinding, KnowledgeDocument, Member, MemberRef,
    MemberRoster, Message, ModerationRule, Page, ScheduleRun, TaskItem,
};
use paths::AppPaths;
use secrets::SecretStore;

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiAutomationInput {
    account_id: String,
    group_id: i64,
    enabled: bool,
    reply: bool,
    tasks: bool,
    recall: bool,
    mute: bool,
    remove: bool,
    manual_takeover: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageQuery {
    account_id: String,
    #[serde(default)]
    group_ids: Vec<i64>,
    keyword: Option<String>,
    kind: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchSendResult {
    group_id: i64,
    success: bool,
    message_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupManagementContext {
    group_id: i64,
    sender_id: i64,
    is_manager: bool,
    capabilities: gateway::GatewayCapabilities,
    member_count: usize,
    announcement_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberBatchInput {
    group_id: i64,
    action: String,
    members: Vec<MemberRef>,
    duration_seconds: Option<i64>,
    nickname: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberBatchResult {
    user_id: Option<i64>,
    nim_id: Option<String>,
    success: bool,
    error: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummarySettings {
    account_id: String,
    enabled: bool,
    time: String,
    group_ids: Vec<i64>,
    timezone: String,
}

pub struct AppState {
    pub paths: AppPaths,
    pub database: Database,
    pub secrets: SecretStore,
    pub gateway: Arc<dyn RuntimeGateway>,
    pub shutdown: Arc<Notify>,
    pub logger: diagnostics::Logger,
}

impl AppState {
    fn initialize() -> AppResult<Self> {
        let paths = AppPaths::discover()?;
        let _ = paths.prepare()?;
        let database = Database::open(&paths)?;
        diagnostics::install_panic_hook(paths.logs.clone());
        let logs = paths.logs.clone();
        // The endpoint is a build-time/runtime-mode decision.  In particular,
        // the public binary never trusts stale database settings or caller URLs.
        let devtools_url = paths.default_devtools_url().to_string();
        let gateway: Arc<dyn RuntimeGateway> =
            Arc::new(CdpGateway::new(CdpClient::new(devtools_url)?));
        Ok(Self {
            secrets: SecretStore::new(paths.secrets.clone()),
            paths,
            database,
            gateway,
            shutdown: Arc::new(Notify::new()),
            logger: diagnostics::Logger::new(logs),
        })
    }
}

#[tauri::command]
fn health(state: State<'_, AppState>) -> Health {
    Health {
        name: "DH BOT",
        version: "3.0.0-beta.1",
        data_dir: state.paths.v3.display().to_string(),
        clean_database: true,
        runtime_mode: state.paths.runtime_mode(),
        build_channel: BuildChannel::CURRENT.name(),
        fixture_available: BuildChannel::CURRENT.fixture_available(),
    }
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn get_runtime_mode(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({
        "mode": state.paths.runtime_mode(),
        "dataDir": state.paths.v3.display().to_string(),
        "restartRequired": false
    })
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn set_runtime_mode(state: State<'_, AppState>, mode: String) -> AppResult<serde_json::Value> {
    state.paths.set_runtime_mode(&mode)?;
    Ok(serde_json::json!({
        "mode": mode.trim().to_ascii_lowercase(),
        "restartRequired": true
    }))
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn start_fixture_host(app: tauri::AppHandle) -> AppResult<String> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::new("fixture_start", error.to_string()))?;
    let directory = executable.parent().unwrap_or(std::path::Path::new("."));
    let resources = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| directory.to_path_buf());
    let names = if cfg!(windows) {
        vec!["DH-Fixture.exe", "dh-fixture.exe"]
    } else {
        vec!["dh-fixture", "DH-Fixture"]
    };
    let mut candidates = Vec::new();
    for name in names {
        candidates.push(resources.join("resources").join("tools").join(name));
        candidates.push(resources.join("tools").join(name));
        candidates.push(directory.join(name));
        candidates.push(directory.join("tools").join(name));
        candidates.push(
            directory
                .join("..")
                .join("Resources")
                .join("tools")
                .join(name),
        );
    }
    let fixture = candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            AppError::new(
                "fixture_start",
                "未找到 DH-Fixture，请确认内部开发包 resources/tools 目录完整",
            )
        })?;
    let mut command = std::process::Command::new(&fixture);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.spawn().map_err(|error| {
        AppError::new("fixture_start", format!("启动 DH-Fixture 失败：{error}"))
    })?;
    Ok(fixture.display().to_string())
}

#[tauri::command]
fn database_status(state: State<'_, AppState>) -> AppResult<DatabaseStatus> {
    state.database.status()
}

#[tauri::command]
async fn diagnose(state: State<'_, AppState>) -> Result<DiagnosticSnapshot, AppError> {
    Ok(state.gateway.diagnose().await)
}

#[tauri::command]
async fn list_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    let (_, account_id) = state.gateway.session_identity().await?;
    let now = Utc::now();
    state.database.upsert_account(&models::Account {
        id: account_id.clone(),
        display_name: account_id.clone(),
        role: "unknown".into(),
        discovered_at: now,
        updated_at: now,
    })?;
    for group in state.gateway.list_groups().await? {
        state.database.upsert_group(&group)?;
    }
    state.database.list_groups(Some(&account_id))
}

#[tauri::command]
fn list_cached_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    state.database.list_groups(None)
}

#[tauri::command]
async fn list_members(state: State<'_, AppState>, group_id: i64) -> AppResult<MemberRoster> {
    let mut roster = state.gateway.list_members(group_id).await?;
    let (self_id, account_id) = state.gateway.session_identity().await?;
    let existing = state.database.list_members(&account_id, group_id)?;
    let had_baseline = !existing.is_empty();
    let mut newly_discovered = std::collections::HashSet::new();
    for member in &mut roster.members {
        if let Some(saved) = existing
            .iter()
            .find(|value| value.user_id == member.user_id)
        {
            member.original_card_name = saved.original_card_name.clone();
            member.managed_card_name = saved.managed_card_name.clone();
            member.card_suffix = saved.card_suffix.clone();
            member.blacklisted = saved.blacklisted;
        } else {
            member.original_card_name = member.card_name.clone();
            newly_discovered.insert(member.user_id);
        }
        state.database.upsert_member(member)?;
    }
    let automatic = state
        .database
        .get_setting(&format!("card.auto.{account_id}.{group_id}"))?
        .as_deref()
        == Some("true");
    if had_baseline && automatic && !newly_discovered.is_empty() {
        let prefix = state
            .database
            .get_setting(&format!("card.prefix.{account_id}.{group_id}"))?
            .unwrap_or_else(|| "DH".into());
        let preview = cardnames::preview(
            group_id,
            &prefix,
            state.database.list_members(&account_id, group_id)?,
            self_id,
        )?;
        for plan in preview.items.into_iter().filter(|plan| {
            plan.status == "planned" && newly_discovered.contains(&plan.member.user_id)
        }) {
            state
                .database
                .enqueue_card_job(&account_id, group_id, &plan, true)?;
        }
    }
    Ok(roster)
}

#[tauri::command]
fn local_members(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<Vec<Member>> {
    state.database.list_members(&account_id, group_id)
}

#[tauri::command]
fn get_ai_settings(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let keys = ["ai.base_url", "ai.webhook_url", "ai.model"];
    let mut values = serde_json::Map::new();
    for key in keys {
        values.insert(
            key.trim_start_matches("ai.").replace('.', "_"),
            serde_json::Value::String(state.database.get_setting(key)?.unwrap_or_default()),
        );
    }
    values.insert(
        "api_key_configured".into(),
        serde_json::Value::Bool(
            !state
                .secrets
                .load()?
                .get("ai.api_key")
                .map(String::is_empty)
                .unwrap_or(true),
        ),
    );
    Ok(serde_json::Value::Object(values))
}

#[tauri::command]
fn get_ai_automation_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let group = state
        .database
        .list_groups(Some(&account_id))?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    let permissions = state.database.group_ai_permissions(&account_id, group_id)?;
    let legacy_permission = |name: &str, default: bool| -> AppResult<bool> {
        Ok(state
            .database
            .get_setting(&format!("ai.permission.{name}.{account_id}.{group_id}"))?
            .map(|value| value == "true")
            .unwrap_or(default))
    };
    Ok(serde_json::json!({
        "enabled": group.enabled,
        "reply": permissions.as_ref().map(|value| value.reply).unwrap_or(group.ai_enabled),
        "tasks": permissions.as_ref().map(|value| value.tasks).unwrap_or(legacy_permission("tasks", true)?),
        "recall": permissions.as_ref().map(|value| value.recall).unwrap_or(legacy_permission("recall", false)?),
        "mute": permissions.as_ref().map(|value| value.mute).unwrap_or(legacy_permission("mute", false)?),
        "remove": permissions.as_ref().map(|value| value.remove).unwrap_or(legacy_permission("remove", false)?),
        "manualTakeover": group.manual_takeover
    }))
}

#[tauri::command]
async fn save_ai_automation_settings(
    state: State<'_, AppState>,
    settings: AiAutomationInput,
) -> AppResult<()> {
    require_manager(&state, settings.group_id).await?;
    let group = state
        .database
        .list_groups(Some(&settings.account_id))?
        .into_iter()
        .find(|group| group.group_id == settings.group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    state.database.set_group_features(
        &settings.account_id,
        settings.group_id,
        settings.enabled,
        settings.reply,
        group.moderation_enabled,
        settings.manual_takeover,
    )?;
    state
        .database
        .save_group_ai_permissions(&GroupAiPermissions {
            account_id: settings.account_id,
            group_id: settings.group_id,
            reply: settings.reply,
            tasks: settings.tasks,
            recall: settings.recall,
            mute: settings.mute,
            remove: settings.remove,
        })?;
    Ok(())
}

#[tauri::command]
fn get_card_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let prefix_key = format!("card.prefix.{account_id}.{group_id}");
    let auto_key = format!("card.auto.{account_id}.{group_id}");
    let paused_key = format!("card.paused.{account_id}.{group_id}");
    Ok(serde_json::json!({
        "prefix": state.database.get_setting(&prefix_key)?.unwrap_or_else(|| "DH".into()),
        "autoRename": state.database.get_setting(&auto_key)?.as_deref() == Some("true"),
        "paused": state.database.get_setting(&paused_key)?.as_deref() == Some("true")
    }))
}

#[tauri::command]
async fn save_card_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    prefix: String,
    auto_rename: bool,
    paused: bool,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    let prefix = if prefix.trim().is_empty() {
        "DH"
    } else {
        prefix.trim()
    };
    state.database.set_setting(
        &format!("card.prefix.{account_id}.{group_id}"),
        prefix,
        false,
    )?;
    state.database.set_setting(
        &format!("card.auto.{account_id}.{group_id}"),
        if auto_rename { "true" } else { "false" },
        false,
    )?;
    state.database.set_setting(
        &format!("card.paused.{account_id}.{group_id}"),
        if paused { "true" } else { "false" },
        false,
    )
}

#[tauri::command]
async fn save_group_welcome(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    welcome_message: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state
        .database
        .set_group_welcome(&account_id, group_id, &welcome_message)
}

#[tauri::command]
fn save_ai_settings(
    state: State<'_, AppState>,
    base_url: String,
    webhook_url: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    if !base_url.trim().is_empty()
        && !base_url.starts_with("https://")
        && !base_url.starts_with("http://127.0.0.1")
        && !base_url.starts_with("http://localhost")
    {
        return Err(AppError::new(
            "url_policy",
            "远程 AI 地址必须使用 HTTPS，本机服务只能使用回环 HTTP",
        ));
    }
    state
        .database
        .set_setting("ai.base_url", base_url.trim(), false)?;
    state
        .database
        .set_setting("ai.webhook_url", webhook_url.trim(), false)?;
    state.database.set_setting(
        "ai.model",
        if model.trim().is_empty() {
            "deepseek-v4-pro"
        } else {
            model.trim()
        },
        false,
    )?;
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        let mut values = state.secrets.load()?;
        values.insert("ai.api_key".into(), api_key);
        state.secrets.save(&values)?;
    }
    Ok(())
}

#[tauri::command]
async fn send_text(state: State<'_, AppState>, group_id: i64, text: String) -> AppResult<String> {
    require_manager(&state, group_id).await?;
    state.gateway.send_text(group_id, &text).await
}

#[tauri::command]
async fn send_text_batch(
    state: State<'_, AppState>,
    group_ids: Vec<i64>,
    text: String,
) -> AppResult<Vec<BatchSendResult>> {
    if text.trim().is_empty() {
        return Err(AppError::new("message_empty", "发送内容不能为空"));
    }
    for group_id in &group_ids {
        require_manager(&state, *group_id).await?;
    }
    let mut results = Vec::with_capacity(group_ids.len());
    for group_id in group_ids {
        match state.gateway.send_text(group_id, text.trim()).await {
            Ok(message_id) => results.push(BatchSendResult {
                group_id,
                success: true,
                message_id,
                error: String::new(),
            }),
            Err(error) => results.push(BatchSendResult {
                group_id,
                success: false,
                message_id: String::new(),
                error: error.message,
            }),
        }
    }
    Ok(results)
}

#[tauri::command]
fn query_messages(state: State<'_, AppState>, query: MessageQuery) -> AppResult<Page<Message>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query
        .cursor
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    let keyword = query.keyword.unwrap_or_default().trim().to_lowercase();
    let kind = query.kind.unwrap_or_default();
    let mut items = state
        .database
        .list_messages(&query.account_id, None, 1000)?
        .into_iter()
        .filter(|message| query.group_ids.is_empty() || query.group_ids.contains(&message.group_id))
        .filter(|message| cursor.map(|value| message.id < value).unwrap_or(true))
        .filter(|message| kind.is_empty() || message.kind == kind)
        .filter(|message| {
            keyword.is_empty()
                || message.text.to_lowercase().contains(&keyword)
                || message.sender_name.to_lowercase().contains(&keyword)
        })
        .take(limit + 1)
        .collect::<Vec<_>>();
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|message| message.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
fn recent_messages(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<Message>> {
    state
        .database
        .list_messages(&account_id, group_id, limit.unwrap_or(100))
}

#[tauri::command]
async fn set_group_features(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    enabled: bool,
    ai_enabled: bool,
    moderation_enabled: bool,
    manual_takeover: bool,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    require_manager(&state, group_id).await?;
    state.database.set_group_features(
        &account_id,
        group_id,
        enabled,
        ai_enabled,
        moderation_enabled,
        manual_takeover,
    )
}

#[tauri::command]
fn list_rules(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<ModerationRule>> {
    state.database.list_rules(&account_id, group_id)
}

#[tauri::command]
async fn save_rule(state: State<'_, AppState>, rule: ModerationRule) -> AppResult<i64> {
    require_account(&state, &rule.account_id).await?;
    if rule.group_id != 0 {
        require_manager(&state, rule.group_id).await?;
    }
    state.database.save_rule(&rule)
}

#[tauri::command]
async fn delete_rule(
    state: State<'_, AppState>,
    account_id: String,
    rule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state.database.delete_rule(&account_id, rule_id)
}

#[tauri::command]
fn export_rules(state: State<'_, AppState>, account_id: String) -> AppResult<String> {
    let rules = state.database.list_rules(&account_id, None)?;
    serde_json::to_string_pretty(&serde_json::json!({"version":1,"rules":rules}))
        .map_err(|error| AppError::new("rules_export", error.to_string()))
}

#[tauri::command]
async fn import_rules(
    state: State<'_, AppState>,
    account_id: String,
    json: String,
) -> AppResult<usize> {
    require_account(&state, &account_id).await?;
    #[derive(Deserialize)]
    struct RulesFile {
        version: i64,
        rules: Vec<ModerationRule>,
    }
    let document: RulesFile = serde_json::from_str(&json)
        .map_err(|error| AppError::new("rules_import", format!("规则 JSON 格式错误：{error}")))?;
    if document.version != 1 {
        return Err(AppError::new(
            "rules_import_version",
            "当前仅支持规则文件 v1",
        ));
    }
    let mut rules = Vec::with_capacity(document.rules.len());
    for mut rule in document.rules {
        rule.id = 0;
        rule.account_id = account_id.clone();
        if !matches!(rule.mode.as_str(), "observe" | "automatic") {
            return Err(AppError::new(
                "rules_import",
                "规则执行模式必须为观察或自动",
            ));
        }
        if rule.group_id != 0 {
            require_manager(&state, rule.group_id).await?;
        }
        rules.push(rule);
    }
    state.database.import_rules(&account_id, &rules)
}

#[tauri::command]
fn list_knowledge_bases(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<KnowledgeBase>> {
    state.database.list_knowledge_bases(&account_id)
}

#[tauri::command]
async fn create_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<i64> {
    require_account(&state, &base.account_id).await?;
    state.database.create_knowledge_base(&base)
}

#[tauri::command]
async fn update_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<()> {
    require_account(&state, &base.account_id).await?;
    state.database.update_knowledge_base(&base)
}

#[tauri::command]
async fn clone_knowledge_base(
    state: State<'_, AppState>,
    account_id: String,
    base_id: i64,
    name: String,
) -> AppResult<i64> {
    require_account(&state, &account_id).await?;
    state
        .database
        .clone_knowledge_base(&account_id, base_id, &name)
}

#[tauri::command]
async fn delete_knowledge_base(
    state: State<'_, AppState>,
    account_id: String,
    base_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state.database.delete_knowledge_base(&account_id, base_id)
}

#[tauri::command]
fn list_knowledge_documents(
    state: State<'_, AppState>,
    base_id: i64,
) -> AppResult<Vec<KnowledgeDocument>> {
    state.database.list_knowledge_documents(base_id)
}

#[tauri::command]
async fn save_knowledge_document(
    state: State<'_, AppState>,
    document: KnowledgeDocument,
) -> AppResult<i64> {
    let account_id = state.database.knowledge_base_account(document.base_id)?;
    require_account(&state, &account_id).await?;
    state.database.upsert_knowledge_document(&document)
}

#[tauri::command]
async fn delete_knowledge_document(
    state: State<'_, AppState>,
    base_id: i64,
    document_id: i64,
) -> AppResult<()> {
    let account_id = state.database.knowledge_base_account(base_id)?;
    require_account(&state, &account_id).await?;
    state
        .database
        .delete_knowledge_document(base_id, document_id)
}

#[tauri::command]
async fn bind_knowledge_base(
    state: State<'_, AppState>,
    base_id: i64,
    account_id: String,
    group_ids: Vec<i64>,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    for group_id in &group_ids {
        require_manager(&state, *group_id).await?;
    }
    state
        .database
        .bind_knowledge_base(base_id, &account_id, &group_ids)
}

#[tauri::command]
fn list_knowledge_bindings(
    state: State<'_, AppState>,
    account_id: String,
    base_id: Option<i64>,
) -> AppResult<Vec<KnowledgeBinding>> {
    state.database.list_knowledge_bindings(&account_id, base_id)
}

#[tauri::command]
fn list_tasks(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<TaskItem>> {
    state.database.list_tasks(&account_id, group_id)
}

#[tauri::command]
async fn save_task(state: State<'_, AppState>, task: TaskItem) -> AppResult<i64> {
    require_account(&state, &task.account_id).await?;
    require_manager(&state, task.group_id).await?;
    state.database.save_task(&task)
}

#[tauri::command]
async fn delete_task(
    state: State<'_, AppState>,
    account_id: String,
    task_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state.database.delete_task(&account_id, task_id)
}

#[tauri::command]
fn list_schedules(state: State<'_, AppState>, account_id: String) -> AppResult<Vec<GroupSchedule>> {
    state.database.list_schedules(&account_id)
}

#[tauri::command]
async fn save_schedule(state: State<'_, AppState>, mut schedule: GroupSchedule) -> AppResult<i64> {
    if schedule.name.trim().is_empty() || schedule.group_ids.is_empty() {
        return Err(AppError::new(
            "schedule_invalid",
            "计划名称和使用群不能为空",
        ));
    }
    require_account(&state, &schedule.account_id).await?;
    for group_id in &schedule.group_ids {
        require_manager(&state, *group_id).await?;
    }
    if schedule.timezone.trim().is_empty() || schedule.timezone == "local" {
        schedule.timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    }
    let _ = scheduler::evaluate_utc(&schedule, schedule.group_ids[0], Utc::now())?;
    state.database.save_schedule(&schedule)
}

#[tauri::command]
async fn delete_schedule(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state.database.delete_schedule(&account_id, schedule_id)
}

#[tauri::command]
fn list_schedule_runs(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<ScheduleRun>> {
    state
        .database
        .list_schedule_runs(&account_id, schedule_id, limit.unwrap_or(100))
}

#[tauri::command]
fn list_audit(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<AuditEvent>> {
    state.database.list_audit(&account_id, limit.unwrap_or(200))
}

#[tauri::command]
fn query_audit(state: State<'_, AppState>, mut query: AuditQuery) -> AppResult<Page<AuditEvent>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    query.limit = Some(limit + 1);
    let mut items = state.database.query_audit(&query)?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|item| item.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
fn export_audit(
    state: State<'_, AppState>,
    mut query: AuditQuery,
    format: String,
) -> AppResult<String> {
    query.cursor = None;
    query.limit = Some(1000);
    let events = state.database.query_audit(&query)?;
    if format.eq_ignore_ascii_case("json") {
        return serde_json::to_string_pretty(&events)
            .map_err(|error| AppError::new("audit_export", error.to_string()));
    }
    let mut output = String::from("时间,群ID,成员ID,事件,级别,操作者,详情\n");
    for event in events {
        let cells = [
            event.created_at.to_rfc3339(),
            event.group_id.to_string(),
            event.user_id.to_string(),
            event.event,
            event.level,
            event.actor,
            event.details,
        ];
        output.push_str(
            &cells
                .into_iter()
                .map(|value| format!("\"{}\"", value.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }
    Ok(output)
}

#[tauri::command]
fn list_daily_summaries(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<DailySummary>> {
    state
        .database
        .list_daily_summaries(&account_id, limit.unwrap_or(30))
}

#[tauri::command]
fn get_summary_settings(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<SummarySettings> {
    let enabled = state
        .database
        .get_setting(&format!("summary.enabled.{account_id}"))?
        .as_deref()
        == Some("true");
    let time = state
        .database
        .get_setting(&format!("summary.time.{account_id}"))?
        .unwrap_or_else(|| "23:00".into());
    let group_ids = state
        .database
        .get_setting(&format!("summary.groups.{account_id}"))?
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    Ok(SummarySettings {
        account_id,
        enabled,
        time,
        group_ids,
        timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
    })
}

#[tauri::command]
async fn save_summary_settings(
    state: State<'_, AppState>,
    settings: SummarySettings,
) -> AppResult<()> {
    require_account(&state, &settings.account_id).await?;
    chrono::NaiveTime::parse_from_str(&settings.time, "%H:%M")
        .map_err(|_| AppError::new("summary_time", "摘要时间必须使用 HH:MM 格式"))?;
    for group_id in &settings.group_ids {
        require_manager(&state, *group_id).await?;
    }
    state.database.set_setting(
        &format!("summary.enabled.{}", settings.account_id),
        if settings.enabled { "true" } else { "false" },
        false,
    )?;
    state.database.set_setting(
        &format!("summary.time.{}", settings.account_id),
        &settings.time,
        false,
    )?;
    state.database.set_setting(
        &format!("summary.groups.{}", settings.account_id),
        &serde_json::to_string(&settings.group_ids)
            .map_err(|error| AppError::new("summary_settings", error.to_string()))?,
        false,
    )
}

#[tauri::command]
async fn generate_daily_summary(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<DailySummary> {
    require_manager(&state, group_id).await?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let messages = state
        .database
        .list_messages(&account_id, Some(group_id), 1000)?
        .into_iter()
        .filter(|message| {
            message
                .sent_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
                == today
        })
        .take(200)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Err(AppError::new(
            "summary_empty",
            "当前群今天还没有可供总结的消息",
        ));
    }
    let base_url = state
        .database
        .get_setting("ai.base_url")?
        .unwrap_or_default();
    let webhook_url = state
        .database
        .get_setting("ai.webhook_url")?
        .unwrap_or_default();
    let model = state
        .database
        .get_setting("ai.model")?
        .unwrap_or_else(|| "deepseek-v4-pro".into());
    let api_key = state
        .secrets
        .load()?
        .get("ai.api_key")
        .cloned()
        .unwrap_or_default();
    let provider = ai::ConfiguredProvider::new(ai::AiConfig {
        base_url,
        webhook_url,
        model,
        api_key,
        timeout: Duration::from_secs(30),
    })?;
    let group_name = state
        .database
        .list_groups(Some(&account_id))?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .map(|group| group.name)
        .unwrap_or_default();
    let request = ai::AiRequest {
        version: "1",
        event_id: uuid::Uuid::new_v4().to_string(),
        persona: ai::PERSONA,
        group_id,
        group_name,
        member_id: 1,
        member_name: "本地摘要任务".into(),
        member_role: "admin".into(),
        message_id: uuid::Uuid::new_v4().to_string(),
        message: "请用简短中文总结今天的重要信息、待办和需要人工确认的事项。摘要只保存在总览，不生成任何群管动作。".into(),
        recent_context: messages
            .into_iter()
            .rev()
            .map(|message| ai::AiContextMessage {
                user_id: message.user_id,
                name: message.sender_name,
                text: message.text,
                time: message.sent_at.timestamp(),
            })
            .collect(),
        knowledge: Vec::new(),
    };
    let decision = ai::AiProvider::decide(&provider, &request).await?;
    let summary = DailySummary {
        id: 0,
        account_id,
        group_id,
        local_date: today,
        content: decision.reply,
        source: "ai".into(),
        created_at: Utc::now(),
    };
    let id = state.database.save_daily_summary(&summary)?;
    Ok(DailySummary { id, ..summary })
}

#[tauri::command]
async fn test_ai(
    state: State<'_, AppState>,
    message: String,
    recent_context: Vec<ai::AiContextMessage>,
) -> AppResult<ai::AiTestResult> {
    let base_url = state
        .database
        .get_setting("ai.base_url")?
        .unwrap_or_default();
    let webhook_url = state
        .database
        .get_setting("ai.webhook_url")?
        .unwrap_or_default();
    let model = state
        .database
        .get_setting("ai.model")?
        .unwrap_or_else(|| "deepseek-v4-pro".into());
    let api_key = state
        .secrets
        .load()?
        .get("ai.api_key")
        .cloned()
        .unwrap_or_default();
    let provider = ai::ConfiguredProvider::new(ai::AiConfig {
        base_url,
        webhook_url,
        model,
        api_key,
        timeout: Duration::from_secs(30),
    })?;
    provider
        .test(&ai::AiRequest::testing(message, recent_context))
        .await
}

#[tauri::command]
async fn test_prediction(message: String) -> AppResult<serde_json::Value> {
    match prediction::fetch(&message).await? {
        Ok(result) => {
            Ok(serde_json::json!({ "result": result, "reply": prediction::format_reply(&result) }))
        }
        Err(prompt) => Ok(serde_json::json!({ "prompt": prompt })),
    }
}

#[tauri::command]
async fn locate_wangshangliao() -> AppResult<Vec<platform::InstallCandidate>> {
    platform::locate().await
}

#[tauri::command]
async fn start_wangshangliao(
    state: State<'_, AppState>,
    path: Option<String>,
    devtools_url: Option<String>,
    confirm_restart: bool,
) -> AppResult<platform::WangStartResult> {
    let _ = devtools_url;
    platform::start(
        path,
        state.paths.default_devtools_url().to_string(),
        confirm_restart,
    )
    .await
}

#[tauri::command]
async fn inspect_wangshangliao(
    state: State<'_, AppState>,
    devtools_url: Option<String>,
) -> AppResult<String> {
    let _ = devtools_url;
    platform::inspect(state.paths.default_devtools_url()).await
}

#[tauri::command]
async fn get_wang_profile_status(
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    platform::profile_status(path).await
}

#[tauri::command]
async fn apply_wang_profile_patch(
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    platform::apply_profile_patch(path).await
}

#[tauri::command]
async fn restore_wang_profile_patch(
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    platform::restore_profile_patch(path).await
}

#[tauri::command]
async fn recall_message(
    state: State<'_, AppState>,
    group_id: i64,
    sender_user_id: i64,
    message_id: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state
        .gateway
        .recall(group_id, sender_user_id, &message_id)
        .await
}

#[tauri::command]
async fn mute_member(
    state: State<'_, AppState>,
    group_id: i64,
    user_id: i64,
    duration_seconds: i64,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state
        .gateway
        .mute(group_id, user_id, duration_seconds)
        .await
}

#[tauri::command]
async fn unmute_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state.gateway.unmute(group_id, user_id).await
}

#[tauri::command]
async fn rename_member(
    state: State<'_, AppState>,
    group_id: i64,
    member: MemberRef,
    nickname: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state.gateway.rename(group_id, &member, &nickname).await
}

#[tauri::command]
async fn remove_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state.gateway.remove_member(group_id, user_id).await
}

#[tauri::command]
async fn set_group_mute(state: State<'_, AppState>, group_id: i64, muted: bool) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state.gateway.set_group_mute(group_id, muted).await
}

#[tauri::command]
async fn get_group_management_context(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<GroupManagementContext> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    let is_manager = roster.members.iter().any(|member| {
        member.user_id == sender_id && matches!(member.role.as_str(), "owner" | "admin")
    });
    Ok(GroupManagementContext {
        group_id,
        sender_id,
        is_manager,
        capabilities: state.gateway.capabilities(),
        member_count: roster.members.len(),
        announcement_status: if state.gateway.capabilities().announcement {
            "可用".into()
        } else {
            "功能未开放".into()
        },
    })
}

#[tauri::command]
async fn execute_member_batch(
    state: State<'_, AppState>,
    input: MemberBatchInput,
) -> AppResult<Vec<MemberBatchResult>> {
    require_manager(&state, input.group_id).await?;
    let action = input.action.trim().to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "mute" | "unmute" | "remove" | "blacklist" | "rename"
    ) {
        return Err(AppError::new("batch_action", "成员批量动作不受支持"));
    }
    if action == "rename" && input.nickname.as_deref().unwrap_or("").trim().is_empty() {
        return Err(AppError::new("batch_action", "批量改名需要填写名称"));
    }
    let account_id = state.gateway.session_identity().await?.1;
    let mut results = Vec::with_capacity(input.members.len());
    for member in input.members {
        let result = match action.as_str() {
            "mute" => {
                state
                    .gateway
                    .mute(
                        input.group_id,
                        member.user_id.unwrap_or_default(),
                        input.duration_seconds.unwrap_or(600),
                    )
                    .await
            }
            "unmute" => {
                state
                    .gateway
                    .unmute(input.group_id, member.user_id.unwrap_or_default())
                    .await
            }
            "remove" => {
                state
                    .gateway
                    .remove_member(input.group_id, member.user_id.unwrap_or_default())
                    .await
            }
            "rename" => {
                state
                    .gateway
                    .rename(
                        input.group_id,
                        &member,
                        input.nickname.as_deref().unwrap_or("").trim(),
                    )
                    .await
            }
            "blacklist" => state.database.set_member_blacklisted(
                &account_id,
                input.group_id,
                member.user_id.unwrap_or_default(),
                true,
            ),
            _ => unreachable!(),
        };
        results.push(MemberBatchResult {
            user_id: member.user_id,
            nim_id: member.nim_id,
            success: result.is_ok(),
            error: result.err().map(|error| error.message).unwrap_or_default(),
        });
    }
    Ok(results)
}

#[tauri::command]
async fn preview_card_names(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    prefix: String,
) -> AppResult<CardPreview> {
    require_manager(&state, group_id).await?;
    let (self_id, _) = state.gateway.session_identity().await?;
    cardnames::preview(
        group_id,
        &prefix,
        state.database.list_members(&account_id, group_id)?,
        self_id,
    )
}

#[tauri::command]
async fn apply_card_names(
    state: State<'_, AppState>,
    group_id: i64,
    plans: Vec<CardPlan>,
) -> AppResult<usize> {
    require_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let mut queued = 0;
    for plan in plans
        .into_iter()
        .filter(|plan| plan.status == "planned" && !plan.suggested_name.is_empty())
    {
        if state
            .database
            .enqueue_card_job(&account_id, group_id, &plan, false)?
        {
            queued += 1;
        }
    }
    Ok(queued)
}

#[tauri::command]
fn list_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    limit: Option<usize>,
) -> AppResult<Vec<CardRenameJob>> {
    state
        .database
        .list_card_jobs(&account_id, group_id, limit.unwrap_or(200))
}

#[tauri::command]
async fn retry_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<usize> {
    require_manager(&state, group_id).await?;
    state.database.retry_failed_card_jobs(&account_id, group_id)
}

async fn require_manager(state: &State<'_, AppState>, group_id: i64) -> AppResult<()> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if roster.members.into_iter().any(|member| {
        member.user_id == sender_id && matches!(member.role.as_str(), "owner" | "admin")
    }) {
        Ok(())
    } else {
        Err(AppError::new(
            "management_required",
            "需要将账号权限设置为管理",
        ))
    }
}

async fn require_account(state: &State<'_, AppState>, account_id: &str) -> AppResult<i64> {
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
    ($($developer:ident),* $(,)?) => {
        tauri::generate_handler![
            health,
            database_status,
            diagnose,
            list_groups,
            list_cached_groups,
            list_members,
            local_members,
            get_ai_settings,
            get_ai_automation_settings,
            save_ai_automation_settings,
            get_card_settings,
            save_card_settings,
            save_group_welcome,
            save_ai_settings,
            send_text,
            send_text_batch,
            query_messages,
            recent_messages,
            set_group_features,
            list_rules,
            save_rule,
            delete_rule,
            export_rules,
            import_rules,
            list_knowledge_bases,
            create_knowledge_base,
            update_knowledge_base,
            clone_knowledge_base,
            delete_knowledge_base,
            list_knowledge_documents,
            save_knowledge_document,
            delete_knowledge_document,
            bind_knowledge_base,
            list_knowledge_bindings,
            list_tasks,
            save_task,
            delete_task,
            list_schedules,
            save_schedule,
            delete_schedule,
            list_schedule_runs,
            list_audit,
            query_audit,
            export_audit,
            list_daily_summaries,
            get_summary_settings,
            save_summary_settings,
            generate_daily_summary,
            test_ai,
            test_prediction,
            locate_wangshangliao,
            start_wangshangliao,
            inspect_wangshangliao,
            get_wang_profile_status,
            apply_wang_profile_patch,
            restore_wang_profile_patch,
            recall_message,
            mute_member,
            unmute_member,
            rename_member,
            remove_member,
            set_group_mute,
            get_group_management_context,
            execute_member_batch,
            preview_card_names,
            apply_card_names,
            list_card_rename_jobs,
            retry_card_rename_jobs,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if platform::run_maintenance_if_requested() {
        return;
    }
    // Register single-instance arbitration before opening SQLite, logs or any
    // background worker. A second launch only restores the existing window.
    let builder =
        tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app)
        }));
    #[cfg(feature = "fixture")]
    let builder = builder.invoke_handler(dh_handlers![
        get_runtime_mode,
        set_runtime_mode,
        start_fixture_host
    ]);
    #[cfg(not(feature = "fixture"))]
    let builder = builder.invoke_handler(dh_handlers![]);
    builder
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
            let shutdown = state.shutdown.clone();
            app.manage(state);
            let app_handle = app.handle().clone();
            let tray_menu = MenuBuilder::new(app)
                .text("show", "显示 DH BOT")
                .text("pause", "暂停全部自动化")
                .separator()
                .text("exit", "退出 DH BOT")
                .build()?;
            if let Some(tray) = app.tray_by_id("main") {
                tray.set_menu(Some(tray_menu))?;
            }
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "show" => {
                    show_main_window(app);
                }
                "pause" => {
                    if let Some(state) = app.try_state::<AppState>() {
                        let current = state
                            .database
                            .get_setting("automation.mode")
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "observe".into());
                        let next = if current == "paused" {
                            "observe"
                        } else {
                            "paused"
                        };
                        let _ = state.database.set_setting("automation.mode", next, false);
                        let _ = app.emit(
                            "automation-paused",
                            serde_json::json!({
                                "paused": next == "paused"
                            }),
                        );
                    }
                }
                "exit" => {
                    if let Some(state) = app.try_state::<AppState>() {
                        state.shutdown.notify_waiters();
                    }
                    app.exit(0);
                }
                _ => {}
            });
            let runtime_gateway: Arc<dyn RuntimeGateway> = app.state::<AppState>().gateway.clone();
            runtime::BackendRuntime::new(
                app.state::<AppState>().database.clone(),
                runtime_gateway,
                app.state::<AppState>().secrets.clone(),
                app.state::<AppState>().shutdown.clone(),
                app.state::<AppState>().logger.clone(),
            )
            .spawn(app_handle.clone());
            bridge::spawn(
                app.state::<AppState>().gateway.clone(),
                app.state::<AppState>().shutdown.clone(),
                app.state::<AppState>().logger.clone(),
            );
            tauri::async_runtime::spawn(async move {
                shutdown.notified().await;
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.close();
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("DH BOT 3.0 failed to start");
}
