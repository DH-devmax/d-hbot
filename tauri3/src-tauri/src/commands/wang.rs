//! 旺商聊定位、启动、维护与网关能力。

use tauri::State;

use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WangStartupSettings {
    path: String,
    auto_start: bool,
}

#[tauri::command]
pub(crate) async fn locate_wangshangliao() -> AppResult<Vec<platform::InstallCandidate>> {
    platform::locate().await
}

#[tauri::command]
pub(crate) async fn get_wang_startup_settings(
    state: State<'_, AppState>,
) -> AppResult<WangStartupSettings> {
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
pub(crate) async fn save_wang_startup_settings(
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
pub(crate) fn take_wang_startup_status(
    state: State<'_, AppState>,
) -> AppResult<Option<WangStartupEvent>> {
    state
        .startup_status
        .lock()
        .map(|mut value| value.take())
        .map_err(|_| AppError::new("wangshangliao_status", "旺商聊启动状态锁已损坏"))
}

#[tauri::command]
pub(crate) async fn start_wangshangliao(
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
pub(crate) async fn focus_wangshangliao(state: State<'_, AppState>) -> AppResult<()> {
    let path = state
        .database_executor
        .get_setting("wangshangliao.path".into())
        .await?
        .filter(|value| !value.trim().is_empty());
    platform::focus(path).await
}

#[tauri::command]
pub(crate) async fn inspect_wangshangliao(
    state: State<'_, AppState>,
    devtools_url: Option<String>,
) -> AppResult<String> {
    let _ = devtools_url;
    platform::inspect(state.paths.default_devtools_url()).await
}

#[tauri::command]
pub(crate) fn get_gateway_capabilities(state: State<'_, AppState>) -> gateway::GatewayCapabilities {
    state.gateway.capabilities()
}

#[tauri::command]
pub(crate) fn get_wang_maintenance_result(
    request_id: String,
) -> AppResult<Option<platform::WangMaintenanceResult>> {
    platform::maintenance_result(&request_id)
}

#[tauri::command]
pub(crate) async fn get_wang_profile_status(
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    platform::profile_status(path).await
}

#[tauri::command]
pub(crate) async fn apply_wang_profile_patch(
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
pub(crate) async fn restore_wang_profile_patch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> AppResult<platform::WangMaintenanceStatus> {
    let status = platform::restore_profile_patch(path).await?;
    let capabilities = state.gateway.calibrate_capabilities("", "");
    let _ = app.emit("gateway-capabilities", capabilities);
    Ok(status)
}
