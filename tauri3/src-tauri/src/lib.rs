#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod bridge;
mod build_channel;
mod business_apps;
#[cfg(feature = "fixture")]
mod calibration;
mod cardnames;
mod contracts;
mod database;
mod defaults;
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
use diagnostics::redact;
use error::{AppError, AppResult};
#[cfg(feature = "fixture")]
use gateway::ConnectionStatus;
use gateway::{CdpClient, CdpGateway, DiagnosticSnapshot, GatewayReceipt, RuntimeGateway};
use models::{
    ActionRecord, AuditEvent, BusinessAppRecord, BusinessAppRun, CardPlan, CardPreview,
    CardRenameJob, DailySummary, Group, GroupAiPermissions, GroupAnnouncement, GroupSchedule,
    KnowledgeBase, KnowledgeBinding, KnowledgeChunk as StoredKnowledgeChunk, KnowledgeDocument,
    Member, MemberRef, MemberRoster, Message, ModerationRule, Page, ScheduleRun, TaskItem,
};
use paths::AppPaths;
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
    processing_state: Option<String>,
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum GroupBatchAction {
    Announcement,
    Mute,
    Unmute,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupBatchInput {
    action: GroupBatchAction,
    group_ids: Vec<i64>,
    text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupBatchResult {
    group_id: i64,
    success: bool,
    status: String,
    request_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WangStartupSettings {
    path: String,
    auto_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WangStartupEvent {
    event_id: String,
    status: String,
    detail: String,
    needs_confirmation: bool,
}

fn wang_startup_event(
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

fn publish_wang_startup_status(
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

async fn persist_wang_installation(
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

async fn clear_wang_installation(database: &DatabaseExecutor) {
    for key in [
        "wangshangliao.path",
        "wangshangliao.path_source",
        "wangshangliao.path_version",
        "wangshangliao.path_last_verified_at",
    ] {
        let _ = database.set_setting(key.into(), String::new(), false).await;
    }
}

async fn resolve_and_persist_wang_installation(
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

async fn reconcile_wang_process_path(
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BusinessAppTestInput {
    account_id: String,
    app_id: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BusinessAppTestResult {
    app_id: String,
    status: String,
    freshness: String,
    reply: String,
    ai_used: bool,
    error: String,
    elapsed_ms: u128,
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
            startup_status: Arc::new(Mutex::new(None)),
            wang_start_lock: Arc::new(tokio::sync::Mutex::new(())),
            logger,
        })
    }
}

fn normalize_close_behavior(value: Option<&str>) -> &'static str {
    match value {
        Some("tray") => "tray",
        Some("exit") => "exit",
        _ => "ask",
    }
}

fn cached_close_behavior(state: &AppState) -> String {
    state
        .close_behavior
        .read()
        .map(|value| normalize_close_behavior(Some(value.as_str())).to_string())
        .unwrap_or_else(|_| "ask".into())
}

fn cache_close_behavior(state: &AppState, value: &str) {
    if let Ok(mut current) = state.close_behavior.write() {
        *current = normalize_close_behavior(Some(value)).to_string();
    }
}

fn wang_auto_start_enabled(value: Option<&str>) -> bool {
    value != Some("false")
}

#[tauri::command]
fn health(state: State<'_, AppState>) -> Health {
    Health {
        name: "DH BOT",
        version: env!("CARGO_PKG_VERSION"),
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
fn start_fixture_host(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<String> {
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
    let (version, script_hash) = contracts::fixture_calibration_identity();
    let capabilities = state.gateway.calibrate_capabilities(&version, &script_hash);
    let _ = app.emit("gateway-capabilities", capabilities);
    Ok(fixture.display().to_string())
}

#[cfg(feature = "fixture")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeveloperCalibrationCaptureResult {
    path: String,
    status: DeveloperCalibrationStatus,
}

#[cfg(feature = "fixture")]
async fn calibration_metadata(state: &State<'_, AppState>) -> AppResult<CalibrationMetadata> {
    if state.paths.runtime_mode() != "real"
        || state.paths.default_devtools_url() != "http://127.0.0.1:9222"
    {
        return Err(AppError::new(
            "calibration_environment",
            "真实校准只能连接真实旺商聊 127.0.0.1:9222，Fixture、9233 和 51300 均不可用",
        ));
    }
    let diagnostic = state.gateway.diagnose().await;
    if diagnostic.status != ConnectionStatus::Ready {
        return Err(AppError::new(
            "calibration_not_ready",
            format!("旺商聊 NIM 尚未就绪：{}", diagnostic.detail),
        ));
    }
    let configured_path = state
        .database_executor
        .get_setting("wangshangliao.path".into())
        .await?
        .filter(|value| !value.trim().is_empty());
    let process = platform::running_process_identity_for(configured_path)
        .await?
        .ok_or_else(|| {
            AppError::new("calibration_process", "没有找到当前 9222 对应的旺商聊进程")
        })?;
    let profile = platform::profile_status(Some(process.image_path)).await?;
    if profile.script_hash.len() != 64 {
        return Err(AppError::new(
            "calibration_script",
            "无法读取旺商聊主脚本 SHA-256，拒绝开始校准",
        ));
    }
    Ok(CalibrationMetadata {
        app_file_version: process.file_version,
        main_script_sha256: profile.script_hash,
        page_title: diagnostic.page_title,
        page_url: diagnostic.page_url,
    })
}

#[cfg(feature = "fixture")]
#[tauri::command]
async fn begin_developer_calibration(
    state: State<'_, AppState>,
    capabilities: Vec<String>,
) -> AppResult<DeveloperCalibrationStatus> {
    let metadata = calibration_metadata(&state).await?;
    state
        .gateway
        .begin_developer_calibration(metadata, capabilities)
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn get_developer_calibration_status(state: State<'_, AppState>) -> DeveloperCalibrationStatus {
    state.gateway.developer_calibration_status()
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn cancel_developer_calibration(state: State<'_, AppState>) -> DeveloperCalibrationStatus {
    state.gateway.cancel_developer_calibration()
}

#[cfg(feature = "fixture")]
#[tauri::command]
fn finish_developer_calibration(
    state: State<'_, AppState>,
    restored: bool,
) -> AppResult<DeveloperCalibrationCaptureResult> {
    let exported = state.gateway.finish_developer_calibration(restored)?;
    let output_directory = state
        .paths
        .root
        .join("developer")
        .join("contracts")
        .join("raw");
    std::fs::create_dir_all(&output_directory).map_err(|error| {
        AppError::new(
            "calibration_write",
            format!("创建本机校准目录失败：{error}"),
        )
    })?;
    let version = exported.status.app_file_version.replace(['.', ' '], "-");
    let file_name = format!(
        "wangshangliao-{version}-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let output = output_directory.join(file_name);
    let serialized = serde_json::to_vec_pretty(&exported.capture).map_err(|error| {
        AppError::new(
            "calibration_serialize",
            format!("序列化 Contract v2 失败：{error}"),
        )
    })?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| {
            AppError::new(
                "calibration_write",
                format!("创建本机 Contract v2 失败：{error}"),
            )
        })?;
    std::io::Write::write_all(&mut file, &serialized).map_err(|error| {
        AppError::new(
            "calibration_write",
            format!("写入本机 Contract v2 失败：{error}"),
        )
    })?;
    std::io::Write::write_all(&mut file, b"\n").map_err(|error| {
        AppError::new(
            "calibration_write",
            format!("完成本机 Contract v2 失败：{error}"),
        )
    })?;
    Ok(DeveloperCalibrationCaptureResult {
        path: output.display().to_string(),
        status: exported.status,
    })
}

#[tauri::command]
async fn database_status(state: State<'_, AppState>) -> AppResult<DatabaseStatus> {
    state.database_executor.status().await
}

#[tauri::command]
async fn get_close_behavior(state: State<'_, AppState>) -> AppResult<String> {
    Ok(cached_close_behavior(&state))
}

#[tauri::command]
async fn reset_close_behavior(state: State<'_, AppState>) -> AppResult<()> {
    state
        .database_executor
        .set_setting("window.close_behavior".into(), "ask".into(), false)
        .await?;
    cache_close_behavior(&state, "ask");
    Ok(())
}

#[tauri::command]
async fn resolve_close_action(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    action: String,
    remember: bool,
) -> AppResult<()> {
    if !matches!(action.as_str(), "tray" | "exit") {
        return Err(AppError::new("close_action", "关闭操作不正确"));
    }
    if remember {
        state
            .database_executor
            .set_setting("window.close_behavior".into(), action.clone(), false)
            .await?;
        cache_close_behavior(&state, &action);
    }
    if action == "tray" {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
        show_tray_notification(&app);
    } else {
        request_graceful_exit(app);
    }
    Ok(())
}

#[tauri::command]
async fn diagnose(state: State<'_, AppState>) -> Result<DiagnosticSnapshot, AppError> {
    Ok(state.gateway.diagnose().await)
}

#[tauri::command]
async fn list_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    let (_, account_id) = state.gateway.session_identity().await?;
    let now = Utc::now();
    state
        .database_executor
        .upsert_account(models::Account {
            id: account_id.clone(),
            display_name: account_id.clone(),
            role: "unknown".into(),
            discovered_at: now,
            updated_at: now,
        })
        .await?;
    let _ = state
        .database_executor
        .ensure_account_defaults(account_id.clone())
        .await;
    for group in state.gateway.list_groups().await? {
        state.database_executor.upsert_group(group).await?;
    }
    state.database_executor.list_groups(Some(account_id)).await
}

#[tauri::command]
async fn list_cached_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    state.database_executor.list_groups(None).await
}

#[tauri::command]
async fn list_members(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: i64,
    refresh: Option<bool>,
) -> AppResult<MemberRoster> {
    let request_epoch = state.gateway.session_epoch();
    let snapshot_started_at = Utc::now();
    if refresh.unwrap_or(false) {
        state.gateway.invalidate_member_cache(group_id).await;
    }
    let mut roster = state.gateway.list_members(group_id).await?;
    let (self_id, account_id) = state.gateway.session_identity().await?;
    let existing = state
        .database_executor
        .list_members(account_id.clone(), group_id)
        .await?;
    let had_baseline = !existing.is_empty();
    let mut newly_discovered = std::collections::HashSet::new();
    let mut present_user_ids = std::collections::HashSet::new();
    for member in &mut roster.members {
        if let Some(saved) = existing.iter().find(|value| {
            value.user_id == member.user_id
                || (!member.nim_id.is_empty() && value.nim_id == member.nim_id)
        }) {
            member.original_card_name = saved.original_card_name.clone();
            member.managed_card_name = saved.managed_card_name.clone();
            member.card_suffix = saved.card_suffix.clone();
            member.blacklisted = saved.blacklisted;
        } else {
            member.original_card_name = member.card_name.clone();
            newly_discovered.insert(member.user_id);
        }
        let canonical_user_id = state
            .database_executor
            .upsert_member(member.clone())
            .await?;
        present_user_ids.insert(canonical_user_id);
    }
    if refresh.unwrap_or(false)
        && roster.authority == "authoritative"
        && request_epoch == state.gateway.session_epoch()
    {
        let _ = state
            .database_executor
            .mark_members_not_present_before(
                account_id.clone(),
                group_id,
                present_user_ids.into_iter().collect(),
                snapshot_started_at,
            )
            .await?;
    }
    let automatic = state
        .database_executor
        .get_setting(format!("card.auto.{account_id}.{group_id}"))
        .await?
        .as_deref()
        == Some("true");
    if had_baseline && automatic && !newly_discovered.is_empty() {
        let prefix = state
            .database_executor
            .get_setting(format!("card.prefix.{account_id}.{group_id}"))
            .await?
            .unwrap_or_else(|| "DH".into());
        let preview_members = state
            .database_executor
            .list_members(account_id.clone(), group_id)
            .await?
            .into_iter()
            .filter(|member| member.present)
            .collect();
        let preview = cardnames::preview(group_id, &prefix, preview_members, self_id)?;
        for plan in preview.items.into_iter().filter(|plan| {
            plan.status == "planned" && newly_discovered.contains(&plan.member.user_id)
        }) {
            state
                .database_executor
                .enqueue_card_job(account_id.clone(), group_id, plan, true)
                .await?;
        }
    }
    let _ = app.emit(
        "member-roster-status",
        serde_json::json!({
            "accountId": account_id,
            "groupId": group_id,
            "status": roster.status.clone(),
            "reportedCount": roster.reported_count,
            "resolvedCount": roster.resolved_count,
            "complete": roster.complete,
            "completenessReason": roster.completeness_reason.clone(),
            "retryAt": roster.retry_at.clone(),
            "canonicalCount": roster.canonical_count,
            "syntheticUserIds": roster.synthetic_user_ids.clone(),
            "sourceErrors": roster.source_errors.clone(),
        }),
    );
    Ok(roster)
}

#[tauri::command]
async fn local_members(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<Vec<Member>> {
    let members = state
        .database_executor
        .list_members(account_id, group_id)
        .await?;
    Ok(members
        .into_iter()
        .filter(|member| member.present)
        .collect())
}

#[tauri::command]
async fn get_ai_settings(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let keys = ["ai.base_url", "ai.webhook_url", "ai.model"];
    let mut values = serde_json::Map::new();
    for key in keys {
        values.insert(
            key.trim_start_matches("ai.").replace('.', "_"),
            serde_json::Value::String(
                state
                    .database_executor
                    .get_setting(key.into())
                    .await?
                    .unwrap_or_default(),
            ),
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
async fn get_ai_automation_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let group = state
        .database_executor
        .list_groups(Some(account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    let permissions = state
        .database_executor
        .group_ai_permissions(account_id.clone(), group_id)
        .await?;
    let legacy_tasks = state
        .database_executor
        .get_setting(format!("ai.permission.tasks.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(true);
    let legacy_recall = state
        .database_executor
        .get_setting(format!("ai.permission.recall.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    let legacy_mute = state
        .database_executor
        .get_setting(format!("ai.permission.mute.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    let legacy_remove = state
        .database_executor
        .get_setting(format!("ai.permission.remove.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    Ok(serde_json::json!({
        "enabled": group.enabled,
        "reply": permissions.as_ref().map(|value| value.reply).unwrap_or(group.ai_enabled),
        "tasks": permissions.as_ref().map(|value| value.tasks).unwrap_or(legacy_tasks),
        "recall": permissions.as_ref().map(|value| value.recall).unwrap_or(legacy_recall),
        "mute": permissions.as_ref().map(|value| value.mute).unwrap_or(legacy_mute),
        "remove": permissions.as_ref().map(|value| value.remove).unwrap_or(legacy_remove),
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
        .database_executor
        .list_groups(Some(settings.account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == settings.group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    state
        .database_executor
        .set_group_features(
            settings.account_id.clone(),
            settings.group_id,
            settings.enabled,
            settings.reply,
            group.moderation_enabled,
            settings.manual_takeover,
        )
        .await?;
    state
        .database_executor
        .save_group_ai_permissions(GroupAiPermissions {
            account_id: settings.account_id,
            group_id: settings.group_id,
            reply: settings.reply,
            tasks: settings.tasks,
            recall: settings.recall,
            mute: settings.mute,
            remove: settings.remove,
        })
        .await?;
    Ok(())
}

#[tauri::command]
async fn get_card_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let prefix_key = format!("card.prefix.{account_id}.{group_id}");
    let auto_key = format!("card.auto.{account_id}.{group_id}");
    let paused_key = format!("card.paused.{account_id}.{group_id}");
    Ok(serde_json::json!({
        "prefix": state.database_executor.get_setting(prefix_key).await?.unwrap_or_else(|| "DH".into()),
        "autoRename": state.database_executor.get_setting(auto_key).await?.as_deref() == Some("true"),
        "paused": state.database_executor.get_setting(paused_key).await?.as_deref() == Some("true")
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
    state
        .database_executor
        .set_setting(
            format!("card.prefix.{account_id}.{group_id}"),
            prefix.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("card.auto.{account_id}.{group_id}"),
            if auto_rename { "true" } else { "false" }.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("card.paused.{account_id}.{group_id}"),
            if paused { "true" } else { "false" }.into(),
            false,
        )
        .await
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
        .database_executor
        .set_group_welcome(account_id, group_id, welcome_message)
        .await
}

#[tauri::command]
async fn save_ai_settings(
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
        .database_executor
        .set_setting("ai.base_url".into(), base_url.trim().into(), false)
        .await?;
    state
        .database_executor
        .set_setting("ai.webhook_url".into(), webhook_url.trim().into(), false)
        .await?;
    state
        .database_executor
        .set_setting(
            "ai.model".into(),
            if model.trim().is_empty() {
                "deepseek-v4-pro"
            } else {
                model.trim()
            }
            .into(),
            false,
        )
        .await?;
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
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.send_text(group_id, &text).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "send_text",
        0,
        "人工发送群消息",
        result,
    )
    .await
    .map(|receipt| receipt.message_id)
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
    let account_id = state.gateway.session_identity().await?.1;
    let mut results = Vec::with_capacity(group_ids.len());
    for group_id in group_ids {
        let result = state.gateway.send_text(group_id, text.trim()).await;
        match archive_manual_gateway_result(
            &state,
            &account_id,
            group_id,
            0,
            "send_text",
            0,
            "人工批量发送群消息",
            result,
        )
        .await
        {
            Ok(receipt) => results.push(BatchSendResult {
                group_id,
                success: true,
                message_id: receipt.message_id,
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

fn validate_group_batch_input(
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

#[tauri::command]
async fn execute_group_batch(
    state: State<'_, AppState>,
    input: GroupBatchInput,
) -> AppResult<Vec<GroupBatchResult>> {
    let (action, group_ids, text) = validate_group_batch_input(input)?;

    let account_id = state.gateway.session_identity().await?.1;
    let (kind, reason) = match action {
        GroupBatchAction::Announcement => ("announcement", "人工批量更新群公告"),
        GroupBatchAction::Mute => ("group_mute", "人工批量全员禁言"),
        GroupBatchAction::Unmute => ("group_mute", "人工批量解除全禁"),
    };
    let mut results = Vec::with_capacity(group_ids.len());

    for group_id in group_ids {
        let gateway_result = async {
            require_manager(&state, group_id).await?;
            match action {
                GroupBatchAction::Announcement => {
                    state.gateway.set_group_announcement(group_id, &text).await
                }
                GroupBatchAction::Mute => state.gateway.set_group_mute(group_id, true).await,
                GroupBatchAction::Unmute => state.gateway.set_group_mute(group_id, false).await,
            }
        }
        .await;

        match archive_manual_gateway_result(
            &state,
            &account_id,
            group_id,
            0,
            kind,
            0,
            reason,
            gateway_result,
        )
        .await
        {
            Ok(receipt) => results.push(GroupBatchResult {
                group_id,
                success: true,
                status: receipt.status,
                request_id: receipt.request_id,
                message_id: receipt.message_id,
                error: String::new(),
            }),
            Err(error) => results.push(GroupBatchResult {
                group_id,
                success: false,
                status: if error.delivery_outcome_unknown() {
                    "unknown".into()
                } else {
                    "failed".into()
                },
                request_id: String::new(),
                message_id: String::new(),
                error: error.message,
            }),
        }
    }

    Ok(results)
}

#[tauri::command]
async fn query_messages(
    state: State<'_, AppState>,
    mut query: MessageQuery,
) -> AppResult<Page<Message>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    query.limit = Some(limit + 1);
    let mut items = state.database_executor.query_messages(query).await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|message| message.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
async fn recent_messages(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<Message>> {
    state
        .database_executor
        .list_messages(account_id, group_id, limit.unwrap_or(100))
        .await
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
    state
        .database_executor
        .set_group_features(
            account_id,
            group_id,
            enabled,
            ai_enabled,
            moderation_enabled,
            manual_takeover,
        )
        .await
}

#[tauri::command]
async fn list_rules(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<ModerationRule>> {
    state
        .database_executor
        .list_rules(account_id, group_id)
        .await
}

#[tauri::command]
async fn save_rule(state: State<'_, AppState>, rule: ModerationRule) -> AppResult<i64> {
    require_account(&state, &rule.account_id).await?;
    if rule.group_id != 0 {
        require_manager(&state, rule.group_id).await?;
    }
    state.database_executor.save_rule(rule).await
}

#[tauri::command]
async fn delete_rule(
    state: State<'_, AppState>,
    account_id: String,
    rule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_rule(account_id, rule_id)
        .await
}

#[tauri::command]
async fn export_rules(state: State<'_, AppState>, account_id: String) -> AppResult<String> {
    let rules = state.database_executor.list_rules(account_id, None).await?;
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
    state
        .database_executor
        .import_rules(account_id, rules)
        .await
}

#[tauri::command]
async fn list_knowledge_bases(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<KnowledgeBase>> {
    state
        .database_executor
        .list_knowledge_bases(account_id)
        .await
}

#[tauri::command]
async fn create_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<i64> {
    require_account(&state, &base.account_id).await?;
    state.database_executor.create_knowledge_base(base).await
}

#[tauri::command]
async fn update_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<()> {
    require_account(&state, &base.account_id).await?;
    state.database_executor.update_knowledge_base(base).await
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
        .database_executor
        .clone_knowledge_base(account_id, base_id, name)
        .await
}

#[tauri::command]
async fn delete_knowledge_base(
    state: State<'_, AppState>,
    account_id: String,
    base_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_knowledge_base(account_id, base_id)
        .await
}

#[tauri::command]
async fn list_knowledge_documents(
    state: State<'_, AppState>,
    base_id: i64,
) -> AppResult<Vec<KnowledgeDocument>> {
    state
        .database_executor
        .list_knowledge_documents(base_id)
        .await
}

#[tauri::command]
async fn save_knowledge_document(
    state: State<'_, AppState>,
    document: KnowledgeDocument,
) -> AppResult<i64> {
    let account_id = state
        .database_executor
        .knowledge_base_account(document.base_id)
        .await?;
    require_account(&state, &account_id).await?;
    let document_id = state
        .database_executor
        .upsert_knowledge_document(document.clone())
        .await?;
    let mut stored_document = document;
    stored_document.id = document_id;
    let chunks = knowledge::chunk_document(&stored_document, 800, 120)
        .into_iter()
        .map(|chunk| StoredKnowledgeChunk {
            id: 0,
            document_id,
            chunk_index: chunk.index as i64,
            token_count: chunk.text.chars().count() as i64,
            content: chunk.text,
            content_hash: chunk.content_hash,
            enabled: stored_document.enabled,
        })
        .collect::<Vec<_>>();
    state
        .database_executor
        .replace_knowledge_chunks(document_id, chunks)
        .await?;
    Ok(document_id)
}

#[tauri::command]
async fn delete_knowledge_document(
    state: State<'_, AppState>,
    base_id: i64,
    document_id: i64,
) -> AppResult<()> {
    let account_id = state
        .database_executor
        .knowledge_base_account(base_id)
        .await?;
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_knowledge_document(base_id, document_id)
        .await
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
        .database_executor
        .bind_knowledge_base(account_id, base_id, group_ids)
        .await
}

#[tauri::command]
async fn list_knowledge_bindings(
    state: State<'_, AppState>,
    account_id: String,
    base_id: Option<i64>,
) -> AppResult<Vec<KnowledgeBinding>> {
    state
        .database_executor
        .list_knowledge_bindings(account_id, base_id)
        .await
}

#[tauri::command]
async fn list_tasks(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<TaskItem>> {
    state
        .database_executor
        .list_tasks(account_id, group_id)
        .await
}

#[tauri::command]
async fn save_task(state: State<'_, AppState>, task: TaskItem) -> AppResult<i64> {
    require_account(&state, &task.account_id).await?;
    require_manager(&state, task.group_id).await?;
    state.database_executor.save_task(task).await
}

#[tauri::command]
async fn delete_task(
    state: State<'_, AppState>,
    account_id: String,
    task_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_task(account_id, task_id)
        .await
}

#[tauri::command]
async fn list_schedules(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<GroupSchedule>> {
    state.database_executor.list_schedules(account_id).await
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
    state.database_executor.save_schedule(schedule).await
}

#[tauri::command]
async fn delete_schedule(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_schedule(account_id, schedule_id)
        .await
}

#[tauri::command]
async fn list_schedule_runs(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<ScheduleRun>> {
    state
        .database_executor
        .list_schedule_runs(account_id, schedule_id, limit.unwrap_or(100))
        .await
}

#[tauri::command]
async fn list_audit(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<AuditEvent>> {
    state
        .database_executor
        .list_audit(account_id, limit.unwrap_or(200))
        .await
}

#[tauri::command]
async fn query_audit(
    state: State<'_, AppState>,
    mut query: AuditQuery,
) -> AppResult<Page<AuditEvent>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    query.limit = Some(limit + 1);
    let mut items = state.database_executor.query_audit(query).await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|item| item.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
async fn export_audit(
    state: State<'_, AppState>,
    mut query: AuditQuery,
    format: String,
) -> AppResult<String> {
    query.cursor = None;
    query.limit = Some(1000);
    let events = state.database_executor.query_audit(query).await?;
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
async fn list_daily_summaries(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<DailySummary>> {
    state
        .database_executor
        .list_daily_summaries(account_id, limit.unwrap_or(30))
        .await
}

#[tauri::command]
async fn get_summary_settings(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<SummarySettings> {
    let enabled = state
        .database_executor
        .get_setting(format!("summary.enabled.{account_id}"))
        .await?
        .as_deref()
        == Some("true");
    let time = state
        .database_executor
        .get_setting(format!("summary.time.{account_id}"))
        .await?
        .unwrap_or_else(|| "23:00".into());
    let group_ids = state
        .database_executor
        .get_setting(format!("summary.groups.{account_id}"))
        .await?
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
    state
        .database_executor
        .set_setting(
            format!("summary.enabled.{}", settings.account_id),
            if settings.enabled { "true" } else { "false" }.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("summary.time.{}", settings.account_id),
            settings.time,
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("summary.groups.{}", settings.account_id),
            serde_json::to_string(&settings.group_ids)
                .map_err(|error| AppError::new("summary_settings", error.to_string()))?,
            false,
        )
        .await
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
        .database_executor
        .list_messages(account_id.clone(), Some(group_id), 1000)
        .await?
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
        .database_executor
        .get_setting("ai.base_url".into())
        .await?
        .unwrap_or_default();
    let webhook_url = state
        .database_executor
        .get_setting("ai.webhook_url".into())
        .await?
        .unwrap_or_default();
    let model = state
        .database_executor
        .get_setting("ai.model".into())
        .await?
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
        .database_executor
        .list_groups(Some(account_id.clone()))
        .await?
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
    let id = state
        .database_executor
        .save_daily_summary(summary.clone())
        .await?;
    Ok(DailySummary { id, ..summary })
}

#[tauri::command]
async fn test_ai(
    state: State<'_, AppState>,
    message: String,
    recent_context: Vec<ai::AiContextMessage>,
    include_built_in_knowledge: Option<bool>,
) -> AppResult<ai::AiTestResult> {
    let base_url = state
        .database_executor
        .get_setting("ai.base_url".into())
        .await?
        .unwrap_or_default();
    let webhook_url = state
        .database_executor
        .get_setting("ai.webhook_url".into())
        .await?
        .unwrap_or_default();
    let model = state
        .database_executor
        .get_setting("ai.model".into())
        .await?
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
        .test(&ai::AiRequest::testing_with_knowledge(
            message,
            recent_context,
            include_built_in_knowledge.unwrap_or(false),
        ))
        .await
}

#[tauri::command]
async fn list_business_apps(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<BusinessAppRecord>> {
    state
        .database_executor
        .ensure_business_apps(account_id.clone())
        .await?;
    state.database_executor.list_business_apps(account_id).await
}

#[tauri::command]
async fn set_business_app_enabled(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
    enabled: bool,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    if app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&app_id));
    }
    state
        .database_executor
        .set_business_app_enabled(account_id, app_id, enabled)
        .await
}

#[tauri::command]
async fn get_business_app_health(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
) -> AppResult<BusinessAppHealth> {
    require_account(&state, &account_id).await?;
    if app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&app_id));
    }
    let source = Arc::new(prediction::HttpPredictionSource::new(Duration::from_secs(
        8,
    ))?);
    let registry = BusinessAppRegistry::new(source);
    let app = registry
        .by_id(&app_id)
        .ok_or_else(|| business_apps::app_not_available(&app_id))?;
    let health = app.health(Utc::now()).await?;
    state
        .database_executor
        .update_business_app_health(
            account_id,
            app_id,
            health.status.clone(),
            health.detail.clone(),
            health.checked_at,
        )
        .await?;
    Ok(health)
}

#[tauri::command]
async fn list_business_app_runs(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<BusinessAppRun>> {
    state
        .database_executor
        .list_business_app_runs(account_id, app_id, limit.unwrap_or(50))
        .await
}

#[tauri::command]
async fn test_business_app(
    state: State<'_, AppState>,
    input: BusinessAppTestInput,
) -> AppResult<BusinessAppTestResult> {
    if input.app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&input.app_id));
    }
    if input.message.trim().is_empty() {
        return Err(AppError::new(
            "business_app_input",
            "请输入一条预测测试内容",
        ));
    }
    let started = std::time::Instant::now();
    let source = Arc::new(prediction::HttpPredictionSource::new(Duration::from_secs(
        8,
    ))?);
    let registry = BusinessAppRegistry::new(source);
    let app = registry
        .by_id(&input.app_id)
        .ok_or_else(|| business_apps::app_not_available(&input.app_id))?;
    let now = Utc::now();
    let group = Group {
        account_id: input.account_id.clone(),
        group_id: 0,
        name: "本地业务应用测试".into(),
        owner_user_id: 1,
        enabled: true,
        ai_enabled: true,
        moderation_enabled: false,
        manual_takeover: false,
        welcome_message: String::new(),
        updated_at: now,
    };
    let member = Member {
        account_id: input.account_id.clone(),
        group_id: 0,
        user_id: 1,
        nim_id: String::new(),
        nickname: "本地测试用户".into(),
        card_name: "本地测试用户".into(),
        original_card_name: "本地测试用户".into(),
        managed_card_name: String::new(),
        card_suffix: String::new(),
        role: "admin".into(),
        account_state: "".into(),
        blacklisted: false,
        present: true,
        join_source: "local-test".into(),
        prompt_read: true,
        locked_card_name: String::new(),
        violation_count: 0,
        discovered_at: now,
        joined_at: Some(now),
        last_seen_at: now,
        updated_at: now,
    };
    let message = Message {
        id: 0,
        account_id: input.account_id.clone(),
        group_id: 0,
        server_message_id: "LOCAL-BUSINESS-APP-TEST".into(),
        sequence: 0,
        user_id: 1,
        sender_name: "本地测试用户".into(),
        kind: "text".into(),
        text: input.message,
        sent_at: now,
        received_at: now,
        processed_at: None,
        acknowledged_at: None,
        processing_state: "pending".into(),
        attempts: 0,
        next_attempt_at: None,
        last_error: String::new(),
        mentions_json: "[]".into(),
        source_kind: Some("local-test".into()),
        flow: Some("inbound".into()),
    };
    let outcome = app
        .run(&BusinessAppContext {
            account_id: &input.account_id,
            group: &group,
            member: &member,
            message: &message,
            now,
        })
        .await?;
    if outcome.app_id != input.app_id {
        return Err(AppError::new(
            "business_app_result",
            "业务应用返回了错误的应用标识",
        ));
    }
    let mut reply = outcome.fallback_reply.clone();
    let mut ai_used = false;
    let mut error = String::new();
    if let Some(data) = outcome.narration.as_ref() {
        let provider = ai::ConfiguredProvider::new(ai::AiConfig {
            base_url: state
                .database_executor
                .get_setting("ai.base_url".into())
                .await?
                .unwrap_or_default(),
            webhook_url: state
                .database_executor
                .get_setting("ai.webhook_url".into())
                .await?
                .unwrap_or_default(),
            model: state
                .database_executor
                .get_setting("ai.model".into())
                .await?
                .unwrap_or_else(|| "deepseek-v4-pro".into()),
            api_key: state
                .secrets
                .load()?
                .get("ai.api_key")
                .cloned()
                .unwrap_or_default(),
            timeout: Duration::from_secs(20),
        })?;
        let request = ai::AiRequest {
            version: "1",
            event_id: uuid::Uuid::new_v4().to_string(),
            persona: ai::PERSONA,
            group_id: 0,
            group_name: group.name.clone(),
            member_id: 1,
            member_name: member.card_name.clone(),
            member_role: member.role.clone(),
            message_id: "LOCAL-BUSINESS-APP-TEST".into(),
            message: business_apps::prediction_narration_prompt(data),
            recent_context: Vec::new(),
            knowledge: Vec::new(),
        };
        match ai::AiProvider::decide(&provider, &request).await {
            Ok(decision) if !decision.reply.trim().is_empty() => {
                reply = decision.reply.trim().into();
                ai_used = true;
            }
            Ok(_) => error = "AI 没有返回文字，已使用应用模板".into(),
            Err(reason) => error = format!("{}；已使用应用模板", reason.message),
        }
    }
    Ok(BusinessAppTestResult {
        app_id: outcome.app_id,
        status: if ai_used {
            "succeeded"
        } else {
            outcome.status.as_str()
        }
        .into(),
        freshness: outcome.freshness,
        reply,
        ai_used,
        error,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

#[tauri::command]
async fn locate_wangshangliao() -> AppResult<Vec<platform::InstallCandidate>> {
    platform::locate().await
}

#[tauri::command]
async fn get_wang_startup_settings(state: State<'_, AppState>) -> AppResult<WangStartupSettings> {
    let path = state
        .database_executor
        .get_setting("wangshangliao.path".into())
        .await?
        .unwrap_or_default();
    let auto_start = wang_auto_start_enabled(
        state
            .database_executor
            .get_setting("wangshangliao.auto_start".into())
            .await?
            .as_deref(),
    );
    Ok(WangStartupSettings { path, auto_start })
}

#[tauri::command]
async fn save_wang_startup_settings(
    state: State<'_, AppState>,
    settings: WangStartupSettings,
) -> AppResult<()> {
    state
        .database_executor
        .set_setting(
            "wangshangliao.path".into(),
            settings.path.trim().to_string(),
            false,
        )
        .await?;
    if settings.path.trim().is_empty() {
        for key in [
            "wangshangliao.path_source",
            "wangshangliao.path_version",
            "wangshangliao.path_last_verified_at",
        ] {
            state
                .database_executor
                .set_setting(key.into(), String::new(), false)
                .await?;
        }
    }
    state
        .database_executor
        .set_setting(
            "wangshangliao.auto_start".into(),
            settings.auto_start.to_string(),
            false,
        )
        .await
}

#[tauri::command]
fn take_wang_startup_status(state: State<'_, AppState>) -> AppResult<Option<WangStartupEvent>> {
    state
        .startup_status
        .lock()
        .map(|mut value| value.take())
        .map_err(|_| AppError::new("wangshangliao_status", "旺商聊启动状态锁已损坏"))
}

#[tauri::command]
async fn start_wangshangliao(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
    devtools_url: Option<String>,
    confirm_restart: bool,
) -> AppResult<platform::WangStartResult> {
    let _ = devtools_url;
    let _start_guard = state.wang_start_lock.lock().await;
    let requested_path = match path.filter(|value| !value.trim().is_empty()) {
        Some(path) => Some(path),
        None => state
            .database_executor
            .get_setting("wangshangliao.path".into())
            .await?
            .filter(|value| !value.trim().is_empty()),
    };
    let candidate = match resolve_and_persist_wang_installation(
        &app,
        &state.database_executor,
        requested_path,
        None,
    )
    .await
    {
        Ok(candidate) => candidate,
        Err(error) => {
            publish_wang_startup_status(
                &app,
                None,
                wang_startup_event(error.code.clone(), error.message.clone(), false),
            );
            return Err(error);
        }
    };
    publish_wang_startup_status(
        &app,
        None,
        wang_startup_event(
            "starting",
            "正在启动旺商聊，并连接 DevTools，请稍候。",
            false,
        ),
    );
    let mut result = match platform::start(
        Some(candidate.path),
        state.paths.default_devtools_url().to_string(),
        confirm_restart,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            publish_wang_startup_status(
                &app,
                None,
                wang_startup_event(error.code.clone(), error.message.clone(), false),
            );
            return Err(error);
        }
    };
    reconcile_wang_process_path(&state.database_executor, &mut result).await;
    publish_wang_startup_status(
        &app,
        None,
        wang_startup_event(
            result.status.clone(),
            result.detail.clone(),
            result.needs_confirmation,
        ),
    );
    if let Some(process) = &result.process {
        let script_hash = platform::profile_status(Some(process.image_path.clone()))
            .await
            .map(|status| status.script_hash)
            .unwrap_or_default();
        let capabilities = state
            .gateway
            .calibrate_capabilities(&process.file_version, &script_hash);
        let _ = app.emit("gateway-capabilities", capabilities);
    }
    Ok(result)
}

#[tauri::command]
async fn focus_wangshangliao(state: State<'_, AppState>) -> AppResult<()> {
    let path = state
        .database_executor
        .get_setting("wangshangliao.path".into())
        .await?
        .filter(|value| !value.trim().is_empty());
    platform::focus(path).await
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
fn get_gateway_capabilities(state: State<'_, AppState>) -> gateway::GatewayCapabilities {
    state.gateway.capabilities()
}

#[tauri::command]
fn get_wang_maintenance_result(
    request_id: String,
) -> AppResult<Option<platform::WangMaintenanceResult>> {
    platform::maintenance_result(&request_id)
}

#[tauri::command]
async fn get_wang_profile_status(
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    platform::profile_status(path).await
}

#[tauri::command]
async fn apply_wang_profile_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    let status = platform::apply_profile_patch(path).await?;
    let capabilities = state.gateway.calibrate_capabilities("", "");
    let _ = app.emit("gateway-capabilities", capabilities);
    Ok(status)
}

#[tauri::command]
async fn restore_wang_profile_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    let status = platform::restore_profile_patch(path).await?;
    let capabilities = state.gateway.calibrate_capabilities("", "");
    let _ = app.emit("gateway-capabilities", capabilities);
    Ok(status)
}

#[tauri::command]
async fn recall_message(
    state: State<'_, AppState>,
    group_id: i64,
    sender_user_id: i64,
    message_id: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .recall(group_id, sender_user_id, &message_id)
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        sender_user_id,
        "recall",
        0,
        "人工撤回消息",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn mute_member(
    state: State<'_, AppState>,
    group_id: i64,
    user_id: i64,
    duration_seconds: i64,
) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .mute(group_id, target.user_id, duration_seconds)
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "mute",
        duration_seconds,
        "人工禁言成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn unmute_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.unmute(group_id, target.user_id).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "unmute",
        0,
        "人工解禁成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn rename_member(
    state: State<'_, AppState>,
    group_id: i64,
    member: MemberRef,
    nickname: String,
) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member_ref(&roster, &member)?;
    let target_ref = MemberRef {
        user_id: Some(target.user_id),
        nim_id: (!target.nim_id.is_empty()).then(|| target.nim_id.clone()),
    };
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.rename(group_id, &target_ref, &nickname).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        target.user_id,
        "rename",
        0,
        "人工修改群名片",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn remove_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.remove_member(group_id, target.user_id).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "remove",
        0,
        "人工移出成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn set_group_mute(state: State<'_, AppState>, group_id: i64, muted: bool) -> AppResult<()> {
    let _roster = require_member_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.set_group_mute(group_id, muted).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "group_mute",
        0,
        if muted {
            "人工关群"
        } else {
            "人工开群"
        },
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
async fn set_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
    text: String,
) -> AppResult<GatewayReceipt> {
    let _roster = require_member_manager(&state, group_id).await?;
    if text.trim().is_empty() {
        return Err(AppError::new("announcement_empty", "群公告内容不能为空"));
    }
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .set_group_announcement(group_id, text.trim())
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "announcement",
        0,
        "人工发布或更新群公告",
        result,
    )
    .await
}

#[tauri::command]
async fn get_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<Option<GroupAnnouncement>> {
    state.gateway.get_group_announcement(group_id).await
}

#[tauri::command]
async fn get_group_management_context(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<GroupManagementContext> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    let is_manager = roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    });
    let capabilities = state.gateway.capabilities();
    let announcement_status = match capabilities.announcement {
        gateway::CapabilityStatus::Supported => "可用",
        gateway::CapabilityStatus::Unverified => "当前版本待校准",
        gateway::CapabilityStatus::Unsupported => "功能未开放",
    };
    Ok(GroupManagementContext {
        group_id,
        sender_id,
        is_manager,
        capabilities,
        member_count: roster.members.len(),
        announcement_status: announcement_status.into(),
    })
}

#[tauri::command]
async fn execute_member_batch(
    state: State<'_, AppState>,
    input: MemberBatchInput,
) -> AppResult<Vec<MemberBatchResult>> {
    let roster = require_member_manager(&state, input.group_id).await?;
    let action = input.action.trim().to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "mute" | "unmute" | "remove" | "blacklist" | "unblacklist" | "rename"
    ) {
        return Err(AppError::new("batch_action", "成员批量动作不受支持"));
    }
    if action == "rename" && input.nickname.as_deref().unwrap_or("").trim().is_empty() {
        return Err(AppError::new("batch_action", "批量改名需要填写名称"));
    }
    let account_id = state.gateway.session_identity().await?.1;
    let mut results = Vec::with_capacity(input.members.len());
    for member in input.members {
        let canonical = canonical_member_ref(&roster, &member)?;
        let user_id = canonical.user_id;
        let canonical_ref = MemberRef {
            user_id: Some(canonical.user_id),
            nim_id: (!canonical.nim_id.is_empty()).then(|| canonical.nim_id.clone()),
        };
        let nim_id = member.nim_id.clone();
        let result = if user_id <= 0 && action != "rename" {
            Err(AppError::new("member_identity", "该成员尚未解析出旺商号"))
        } else {
            match action.as_str() {
                "mute" => {
                    state
                        .gateway
                        .mute(
                            input.group_id,
                            user_id,
                            input.duration_seconds.unwrap_or(600),
                        )
                        .await
                }
                "unmute" => state.gateway.unmute(input.group_id, user_id).await,
                "remove" => state.gateway.remove_member(input.group_id, user_id).await,
                "rename" => {
                    state
                        .gateway
                        .rename(
                            input.group_id,
                            &canonical_ref,
                            input.nickname.as_deref().unwrap_or("").trim(),
                        )
                        .await
                }
                "blacklist" => state
                    .database_executor
                    .set_member_blacklisted(account_id.clone(), input.group_id, user_id, true)
                    .await
                    .map(|_| GatewayReceipt::succeeded("database.member.blacklist")),
                "unblacklist" => state
                    .database_executor
                    .set_member_blacklisted(account_id.clone(), input.group_id, user_id, false)
                    .await
                    .map(|_| GatewayReceipt::succeeded("database.member.unblacklist")),
                _ => unreachable!(),
            }
        };
        let archived = archive_manual_gateway_result(
            &state,
            &account_id,
            input.group_id,
            user_id,
            &action,
            input.duration_seconds.unwrap_or(0),
            "人工批量成员操作",
            result,
        )
        .await;
        results.push(MemberBatchResult {
            user_id: member.user_id,
            nim_id,
            success: archived.is_ok(),
            error: archived
                .err()
                .map(|error| error.message)
                .unwrap_or_default(),
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
        state
            .database_executor
            .list_members(account_id, group_id)
            .await?,
        self_id,
    )
}

#[tauri::command]
async fn apply_card_names(
    state: State<'_, AppState>,
    group_id: i64,
    plans: Vec<CardPlan>,
) -> AppResult<usize> {
    let roster = require_member_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let mut queued = 0;
    for mut plan in plans
        .into_iter()
        .filter(|plan| plan.status == "planned" && !plan.suggested_name.is_empty())
    {
        plan.member = canonical_member(&roster, plan.member.user_id)?;
        if state
            .database_executor
            .enqueue_card_job(account_id.clone(), group_id, plan, false)
            .await?
        {
            queued += 1;
        }
    }
    Ok(queued)
}

#[tauri::command]
async fn list_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    limit: Option<usize>,
) -> AppResult<Vec<CardRenameJob>> {
    state
        .database_executor
        .list_card_jobs(account_id, group_id, limit.unwrap_or(200))
        .await
}

#[tauri::command]
async fn retry_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<usize> {
    require_member_manager(&state, group_id).await?;
    let current_account = state.gateway.session_identity().await?.1;
    if current_account != account_id {
        return Err(AppError::new(
            "account_mismatch",
            "只能重试当前已登录账号的名片任务",
        ));
    }
    state
        .database_executor
        .retry_failed_card_jobs(account_id, group_id)
        .await
}

fn redact_archived_receipt(mut receipt: GatewayReceipt) -> GatewayReceipt {
    receipt.route = redact(&receipt.route);
    receipt.status = redact(&receipt.status);
    receipt.business_message = redact(&receipt.business_message);
    receipt.request_id = redact(&receipt.request_id);
    receipt.message_id = redact(&receipt.message_id);
    receipt.session = redact(&receipt.session);
    receipt
}

fn manual_event_name(kind: &str) -> &'static str {
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
        "announcement" => "人工更新群公告",
        _ => "人工群管操作",
    }
}

#[allow(clippy::too_many_arguments)]
async fn archive_manual_gateway_result(
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

async fn require_manager(state: &State<'_, AppState>, group_id: i64) -> AppResult<()> {
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

async fn require_member_manager(
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

fn canonical_member(roster: &MemberRoster, user_id: i64) -> AppResult<Member> {
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

fn canonical_member_ref(roster: &MemberRoster, requested: &MemberRef) -> AppResult<Member> {
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
            get_close_behavior,
            reset_close_behavior,
            resolve_close_action,
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
            execute_group_batch,
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
            list_business_apps,
            set_business_app_enabled,
            get_business_app_health,
            list_business_app_runs,
            test_business_app,
            locate_wangshangliao,
            get_wang_startup_settings,
            save_wang_startup_settings,
            take_wang_startup_status,
            start_wangshangliao,
            focus_wangshangliao,
            inspect_wangshangliao,
            get_gateway_capabilities,
            get_wang_maintenance_result,
            get_wang_profile_status,
            apply_wang_profile_patch,
            restore_wang_profile_patch,
            recall_message,
            mute_member,
            unmute_member,
            rename_member,
            remove_member,
            set_group_mute,
            set_group_announcement,
            get_group_announcement,
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

fn show_tray_notification(app: &tauri::AppHandle) {
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

fn request_graceful_exit(app: tauri::AppHandle) {
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
        if identity != calibrated_identity {
            let capabilities = match &identity {
                Some((version, script_hash)) => {
                    gateway.calibrate_capabilities(version, script_hash)
                }
                None => gateway.calibrate_capabilities("", ""),
            };
            let _ = app.emit("gateway-capabilities", capabilities);
            calibrated_identity = identity;
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
            last_problem = None;
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
        get_runtime_mode,
        set_runtime_mode,
        start_fixture_host,
        begin_developer_calibration,
        get_developer_calibration_status,
        cancel_developer_calibration,
        finish_developer_calibration
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
            let mut runtime_tasks = runtime::BackendRuntime::new(
                app.state::<AppState>().database_executor.clone(),
                runtime_gateway,
                app.state::<AppState>().secrets.clone(),
                app.state::<AppState>().shutdown.clone(),
                app.state::<AppState>().logger.clone(),
            )
            .spawn(app_handle.clone());
            runtime_tasks.push(bridge::spawn(
                app.state::<AppState>().gateway.clone(),
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
        manual_event_name, normalize_close_behavior, redact_archived_receipt,
        validate_group_batch_input, wang_auto_start_enabled, GroupBatchAction, GroupBatchInput,
    };
    use crate::gateway::GatewayReceipt;

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
}
