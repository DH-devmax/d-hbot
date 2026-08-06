//! 开发者通道：运行模式、Fixture 宿主与协议校准。
//!
//! 整个模块由 `commands/mod.rs` 的 `#[cfg(feature = "fixture")]` 统一门控，
//! 生产构建不包含本文件的任何符号。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) fn get_runtime_mode(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({
        "mode": state.paths.runtime_mode(),
        "dataDir": state.paths.v3.display().to_string(),
        "restartRequired": false
    })
}

#[tauri::command]
pub(crate) fn set_runtime_mode(state: State<'_, AppState>, mode: String) -> AppResult<serde_json::Value> {
    state.paths.set_runtime_mode(&mode)?;
    Ok(serde_json::json!({
        "mode": mode.trim().to_ascii_lowercase(),
        "restartRequired": true
    }))
}

#[tauri::command]
pub(crate) fn start_fixture_host(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<String> {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeveloperCalibrationCaptureResult {
    path: String,
    status: DeveloperCalibrationStatus,
}

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

#[tauri::command]
pub(crate) async fn begin_developer_calibration(
    state: State<'_, AppState>,
    capabilities: Vec<String>,
) -> AppResult<DeveloperCalibrationStatus> {
    let metadata = calibration_metadata(&state).await?;
    state
        .gateway
        .begin_developer_calibration(metadata, capabilities)
        .await
}

#[tauri::command]
pub(crate) fn get_developer_calibration_status(state: State<'_, AppState>) -> DeveloperCalibrationStatus {
    state.gateway.developer_calibration_status()
}

#[tauri::command]
pub(crate) fn cancel_developer_calibration(
    state: State<'_, AppState>,
) -> AppResult<DeveloperCalibrationStatus> {
    state.gateway.cancel_developer_calibration()
}

#[tauri::command]
pub(crate) async fn finish_developer_calibration(
    state: State<'_, AppState>,
    restored: bool,
) -> AppResult<DeveloperCalibrationCaptureResult> {
    let finalization = state
        .gateway
        .begin_developer_calibration_finalization()
        .await?;
    state
        .gateway
        .verify_developer_calibration_restoration()
        .await?;
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
        "wangshangliao-{version}-{}-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S-%3f"),
        &uuid::Uuid::new_v4().simple().to_string()[..8],
    );
    let output = output_directory.join(file_name);
    let temporary = output.with_extension("json.tmp");
    let serialized = serde_json::to_vec_pretty(&exported.capture).map_err(|error| {
        AppError::new(
            "calibration_serialize",
            format!("序列化 Contract v2 失败：{error}"),
        )
    })?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
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
    file.sync_all().map_err(|error| {
        AppError::new(
            "calibration_write",
            format!("刷新本机 Contract v2 到磁盘失败：{error}"),
        )
    })?;
    drop(file);
    std::fs::rename(&temporary, &output).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        AppError::new(
            "calibration_write",
            format!("提交本机 Contract v2 原子文件失败：{error}"),
        )
    })?;
    finalization.commit()?;
    Ok(DeveloperCalibrationCaptureResult {
        path: output.display().to_string(),
        status: exported.status,
    })
}
