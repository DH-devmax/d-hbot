//! 系统健康、诊断包、关闭行为与运行项状态。

use tauri::State;

use crate::*;

fn cache_close_behavior(state: &AppState, value: &str) {
    if let Ok(mut current) = state.close_behavior.write() {
        *current = normalize_close_behavior(Some(value)).to_string();
    }
}

#[tauri::command]
pub(crate) fn health(state: State<'_, AppState>) -> Health {
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

#[tauri::command]
pub(crate) fn get_runtime_work_snapshot(state: State<'_, AppState>) -> RuntimeWorkSnapshot {
    state.runtime_coordination.tracker.snapshot()
}

#[tauri::command]
pub(crate) fn acknowledge_runtime_work_failures(state: State<'_, AppState>, ids: Vec<String>) {
    state
        .runtime_coordination
        .tracker
        .acknowledge_failures(&ids);
}

#[tauri::command]
pub(crate) async fn database_status(state: State<'_, AppState>) -> AppResult<DatabaseStatus> {
    state.database_executor.status().await
}

#[tauri::command]
pub(crate) async fn export_support_bundle(
    state: State<'_, AppState>,
) -> AppResult<SupportBundleResult> {
    let database = state.database_executor.status().await?;
    let audits = state.database_executor.list_support_audit(200).await?;
    let diagnostic = state.gateway.diagnose().await;
    let capabilities = state.gateway.capabilities();
    let dispatch_stats = state.runtime_coordination.dispatch_stats.snapshot();
    let paths = state.paths.clone();
    let result = tokio::task::spawn_blocking(move || {
        diagnostics::create_support_bundle(
            &paths,
            database,
            audits,
            diagnostic,
            capabilities,
            dispatch_stats,
        )
    })
    .await
    .map_err(|error| AppError::new("support_bundle", format!("生成诊断包任务异常：{error}")))??;
    state.logger.write(
        "INFO",
        &format!(
            "已生成可分享诊断包：{}，包含 {} 个脱敏文件",
            result.path, result.included_files
        ),
    );
    Ok(result)
}

#[tauri::command]
pub(crate) async fn get_close_behavior(state: State<'_, AppState>) -> AppResult<String> {
    Ok(cached_close_behavior(&state))
}

#[tauri::command]
pub(crate) async fn reset_close_behavior(state: State<'_, AppState>) -> AppResult<()> {
    state
        .database_executor
        .set_setting("window.close_behavior".into(), "ask".into(), false)
        .await?;
    cache_close_behavior(&state, "ask");
    Ok(())
}

#[tauri::command]
pub(crate) async fn resolve_close_action(
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
pub(crate) async fn diagnose(state: State<'_, AppState>) -> Result<DiagnosticSnapshot, AppError> {
    Ok(state.gateway.diagnose().await)
}
