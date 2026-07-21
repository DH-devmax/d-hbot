#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(any(test, windows))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::time::{Duration, SystemTime};
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

use serde::Serialize;

#[cfg(windows)]
use crate::error::AppError;
use crate::error::AppResult;
use crate::gateway::{CdpClient, ConnectionStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallCandidate {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRef {
    pub pid: u32,
    pub image_path: String,
    pub started_at: String,
    pub creation_time: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WangMaintenanceStatus {
    pub state: String,
    pub script_hash: String,
    pub backup_path: Option<String>,
    pub requires_elevation: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WangStartResult {
    pub status: String,
    pub detail: String,
    pub needs_confirmation: bool,
    pub process: Option<ProcessRef>,
}

pub async fn inspect(devtools_url: &str) -> AppResult<String> {
    let client = CdpClient::new(devtools_url.to_string())?;
    Ok(match client.diagnose().await.status {
        ConnectionStatus::Ready => "ready",
        ConnectionStatus::DevToolsReady => "devtools-ready",
        ConnectionStatus::NimNotReady => "nim-not-ready",
        ConnectionStatus::OtherService => "other-service",
        ConnectionStatus::Unavailable => "unavailable",
    }
    .into())
}

#[cfg(not(windows))]
pub async fn locate() -> AppResult<Vec<InstallCandidate>> {
    Ok(Vec::new())
}

#[cfg(not(windows))]
pub async fn start(
    _path: Option<String>,
    _devtools: String,
    _confirm_restart: bool,
) -> AppResult<WangStartResult> {
    Ok(WangStartResult {
        status: "unsupported".into(),
        detail:
            "旺商聊自动启动和进程控制仅在 Windows 可用；macOS 请手动启动旺商聊并打开 DevTools。"
                .into(),
        needs_confirmation: false,
        process: None,
    })
}

#[cfg(not(windows))]
pub async fn profile_status(_path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    Ok(WangMaintenanceStatus {
        state: "unsupported".into(),
        script_hash: String::new(),
        backup_path: None,
        requires_elevation: false,
        detail: "旺商聊启动脚本维护仅在 Windows 可用。".into(),
    })
}

#[cfg(not(windows))]
pub async fn apply_profile_patch(_path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    profile_status(None).await
}

#[cfg(not(windows))]
pub async fn restore_profile_patch(_path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    profile_status(None).await
}

#[cfg(not(windows))]
pub fn run_maintenance_if_requested() -> bool {
    false
}

#[cfg(windows)]
pub async fn locate() -> AppResult<Vec<InstallCandidate>> {
    let mut candidates = Vec::new();
    for root in [
        std::env::var("ProgramFiles").ok(),
        std::env::var("ProgramFiles(x86)").ok(),
        std::env::var("LOCALAPPDATA").ok(),
    ]
    .into_iter()
    .flatten()
    {
        for relative in [
            "wangshangliao_win_online/wangshangliao_win_online.exe",
            "WangShangLiao/WangShangLiao.exe",
            "旺商聊/旺商聊.exe",
        ] {
            let path = PathBuf::from(&root).join(relative);
            if path.is_file() {
                candidates.push(InstallCandidate {
                    path: path.to_string_lossy().into(),
                    source: "常见安装目录".into(),
                });
            }
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path.eq_ignore_ascii_case(&right.path));
    Ok(candidates)
}

#[cfg(windows)]
pub async fn start(
    path: Option<String>,
    devtools: String,
    confirm_restart: bool,
) -> AppResult<WangStartResult> {
    use std::os::windows::process::CommandExt;
    let image_path = if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
        PathBuf::from(path)
    } else {
        locate()
            .await?
            .first()
            .map(|candidate| PathBuf::from(&candidate.path))
            .ok_or_else(|| AppError::new("wangshangliao_not_found", "没有找到旺商聊安装路径"))?
    };
    let image_path = image_path.canonicalize().map_err(|error| {
        AppError::new("wangshangliao_path", format!("旺商聊路径不可用：{error}"))
    })?;
    let port = devtools
        .rsplit(':')
        .next()
        .unwrap_or("9222")
        .trim_end_matches('/');
    let status = inspect(&devtools)
        .await
        .unwrap_or_else(|_| "unavailable".into());
    if matches!(
        status.as_str(),
        "ready" | "devtools-ready" | "nim-not-ready"
    ) {
        let processes = list_processes(&image_path)?;
        if let Some(process) = processes.first() {
            activate(process.pid)?;
            return Ok(WangStartResult {
                status,
                detail:
                    "旺商聊已打开，并已恢复到前台；如果 NIM 尚未就绪，请在旺商聊完成登录后稍候。"
                        .into(),
                needs_confirmation: false,
                process: Some(process.clone()),
            });
        }
    }
    let processes = list_processes(&image_path)?;
    if !processes.is_empty() && !confirm_restart {
        return Ok(WangStartResult {
            status: "running-without-devtools".into(),
            detail: "检测到旺商聊已运行但没有 9222 DevTools，需要确认后重启。".into(),
            needs_confirmation: true,
            process: processes.first().cloned(),
        });
    }
    if confirm_restart {
        for process in &processes {
            stop_process(process)?;
        }
    }
    let profile_detail = match prepare_persistent_profile(&image_path) {
        Ok(detail) => detail,
        Err(error) if error.code == "profile_permission" => {
            request_profile_elevation(Some(image_path.to_string_lossy().into()))?;
            return Ok(WangStartResult {
                status: "maintenance-required".into(),
                detail: "旺商聊安装目录需要管理员权限，已请求同一个 DH-BOT 进程执行维护；维护完成后请再次启动旺商聊。".into(),
                needs_confirmation: false,
                process: None,
            });
        }
        Err(error) => return Err(error),
    };
    let mut command = std::process::Command::new(&image_path);
    command
        .current_dir(image_path.parent().unwrap_or(Path::new(".")))
        .args([
            format!("--remote-debugging-port={port}"),
            format!("--remote-allow-origins=http://127.0.0.1:{port}"),
        ])
        .env("DH_WSL_PARTITION", "dh-primary")
        .creation_flags(0x08000000);
    let child = command.spawn().map_err(|error| {
        AppError::new("wangshangliao_start", format!("启动旺商聊失败：{error}"))
    })?;
    let started = ProcessRef {
        pid: child.id(),
        image_path: image_path.to_string_lossy().into(),
        started_at: format_system_time(SystemTime::now()),
        creation_time: format_system_time(SystemTime::now()),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if inspect(&devtools).await.unwrap_or_default() == "ready" {
            activate(started.pid)?;
            return Ok(WangStartResult {
                status: "ready".into(),
                detail: format!(
                    "旺商聊已启动并恢复到前台。{profile_detail}请完成登录后等待 NIM 初始化。"
                ),
                needs_confirmation: false,
                process: Some(started),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(WangStartResult {
        status: "started-waiting".into(),
        detail: format!("旺商聊进程已启动，但 DevTools 尚未响应。{profile_detail}"),
        needs_confirmation: false,
        process: Some(started),
    })
}

#[cfg(windows)]
fn prepare_persistent_profile(image_path: &Path) -> AppResult<String> {
    let Some(main_dir) = image_path.parent().map(|parent| {
        parent
            .join("resources")
            .join("app")
            .join("dist-electron")
            .join("main")
    }) else {
        return Ok("未找到旺商聊资源目录，保留现有登录分区。".into());
    };
    let script_path = main_dir.join("index.js");
    let original = match std::fs::read(&script_path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("未找到启动脚本，保留现有登录分区。".into())
        }
        Err(error) => {
            return Err(AppError::new(
                "profile_read",
                format!("读取旺商聊启动脚本失败：{error}"),
            ))
        }
    };
    let target = b"persist:${Date.now()}";
    let occurrences = original
        .windows(target.len())
        .filter(|window| *window == target)
        .count();
    if occurrences == 0 {
        return Ok("未检测到随机登录分区代码，保留现有登录分区。".into());
    }
    if occurrences != 1 {
        return Err(AppError::new(
            "profile_unknown_version",
            "旺商聊启动脚本版本与已知格式不一致，已停止修改。",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(&original);
    let hash = format!("{:x}", hasher.finalize());
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("profile_path", "找不到 Windows APPDATA 目录"))?;
    let backup_dir = appdata.join("DH").join("3.0").join("wangshangliao-backups");
    std::fs::create_dir_all(&backup_dir).map_err(|error| {
        AppError::new(
            "profile_backup",
            format!("创建启动脚本备份目录失败：{error}"),
        )
    })?;
    let backup_path = backup_dir.join(format!("index.js.{hash}.bak"));
    if !backup_path.exists() {
        std::fs::write(&backup_path, &original).map_err(|error| {
            AppError::new("profile_backup", format!("备份旺商聊启动脚本失败：{error}"))
        })?;
    }
    let replacement = b"persist:dh-primary";
    let mut patched = Vec::with_capacity(original.len() - target.len() + replacement.len());
    if let Some(index) = original
        .windows(target.len())
        .position(|window| window == target)
    {
        patched.extend_from_slice(&original[..index]);
        patched.extend_from_slice(replacement);
        patched.extend_from_slice(&original[index + target.len()..]);
    }
    let temporary = script_path.with_extension("js.dh-tmp");
    std::fs::write(&temporary, patched).map_err(|error| {
        AppError::new("profile_write", format!("写入旺商聊启动脚本失败：{error}"))
    })?;
    if let Err(error) = std::fs::rename(&temporary, &script_path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(AppError::new(
            "profile_permission",
            format!("旺商聊安装目录没有写入权限，请以管理员身份运行 DH BOT 一次：{error}"),
        ));
    }
    Ok("已备份并启用固定登录分区 dh-primary。".into())
}

#[cfg(windows)]
pub async fn profile_status(path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    let image_path = resolve_image_path(path).await?;
    let script_path = profile_script_path(&image_path);
    if !script_path.is_file() {
        return Ok(WangMaintenanceStatus {
            state: "script-missing".into(),
            script_hash: String::new(),
            backup_path: None,
            requires_elevation: false,
            detail: "没有找到旺商聊启动脚本，当前版本暂不支持固定登录分区。".into(),
        });
    }
    let content = std::fs::read(&script_path).map_err(|error| {
        AppError::new("profile_read", format!("读取旺商聊启动脚本失败：{error}"))
    })?;
    let script_hash = sha256_hex(&content);
    let original_marker = b"persist:${Date.now()}";
    let patched_marker = b"persist:dh-primary";
    let original_count = count_bytes(&content, original_marker);
    let patched_count = count_bytes(&content, patched_marker);
    let (state, detail) = if patched_count == 1 && original_count == 0 {
        ("patched", "已启用固定登录分区 dh-primary。")
    } else if original_count == 1 && patched_count == 0 {
        (
            "maintenance-required",
            "旺商聊仍使用随机登录分区，需要应用固定分区维护。",
        )
    } else {
        (
            "unknown-version",
            "启动脚本结构与已知版本不一致，已保持原文件。",
        )
    };
    let backup_path = latest_profile_backup()?.map(|path| path.to_string_lossy().into());
    let requires_elevation = std::fs::OpenOptions::new()
        .write(true)
        .open(&script_path)
        .map(|_| false)
        .unwrap_or_else(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
    Ok(WangMaintenanceStatus {
        state: state.into(),
        script_hash,
        backup_path,
        requires_elevation,
        detail: detail.into(),
    })
}

#[cfg(windows)]
pub async fn apply_profile_patch(path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    let image_path = resolve_image_path(path.clone()).await?;
    match prepare_persistent_profile(&image_path) {
        Ok(_) => profile_status(path).await,
        Err(error) if error.code == "profile_permission" => {
            if maintenance_arguments_present() {
                return Err(error);
            }
            request_profile_elevation(path.clone())?;
            let mut status = profile_status(path).await?;
            status.state = "elevation-requested".into();
            status.requires_elevation = true;
            status.detail =
                "旺商聊安装目录需要管理员权限，已请求同一个 DH-BOT 进程以管理员身份执行维护。"
                    .into();
            Ok(status)
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub async fn restore_profile_patch(path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    let image_path = resolve_image_path(path.clone()).await?;
    let script_path = profile_script_path(&image_path);
    let backup = latest_profile_backup()?.ok_or_else(|| {
        AppError::new(
            "profile_backup_missing",
            "没有找到可恢复的旺商聊启动脚本备份",
        )
    })?;
    let original = std::fs::read(&backup).map_err(|error| {
        AppError::new(
            "profile_backup",
            format!("读取旺商聊启动脚本备份失败：{error}"),
        )
    })?;
    if count_bytes(&original, b"persist:${Date.now()}") != 1 {
        return Err(AppError::new(
            "profile_backup_invalid",
            "启动脚本备份结构校验失败，已保持当前文件",
        ));
    }
    atomic_replace(&script_path, &original).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "profile_permission"
        } else {
            "profile_restore"
        };
        AppError::new(code, format!("恢复旺商聊启动脚本失败：{error}"))
    })?;
    profile_status(path).await
}

#[cfg(windows)]
pub fn request_profile_elevation(path: Option<String>) -> AppResult<()> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::new("maintenance_uac", error.to_string()))?;
    let image = path.unwrap_or_default();
    let args = format!(
        "--maintenance apply --wang-path \"{}\"",
        image.replace('"', "")
    );
    let verb = wide("runas");
    let file = wide(executable.as_os_str());
    let parameters = wide(&args);
    let directory = executable
        .parent()
        .map(|value| wide(value.as_os_str()))
        .unwrap_or_default();
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let result = unsafe {
        ShellExecuteW(
            0 as HWND,
            verb.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            directory.as_ptr(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize <= 32 {
        return Err(AppError::new("maintenance_uac", "请求管理员维护权限失败"));
    }
    Ok(())
}

#[cfg(windows)]
pub fn run_maintenance_if_requested() -> bool {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--maintenance") {
        return false;
    }
    let operation = args.next().unwrap_or_default();
    let mut path = None;
    while let Some(argument) = args.next() {
        if argument == "--wang-path" {
            path = args.next();
        }
    }
    let result = match operation.as_str() {
        "apply" => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("maintenance runtime")
            .block_on(apply_profile_patch(path)),
        "restore" => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("maintenance runtime")
            .block_on(restore_profile_patch(path)),
        _ => Err(AppError::new("maintenance", "未知维护操作")),
    };
    if let Err(error) = result {
        eprintln!("DH BOT 维护失败：{}", error.message);
        std::process::exit(1);
    }
    true
}

#[cfg(windows)]
fn maintenance_arguments_present() -> bool {
    std::env::args().any(|argument| argument == "--maintenance")
}

#[cfg(windows)]
fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
async fn resolve_image_path(path: Option<String>) -> AppResult<PathBuf> {
    let value = if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
        PathBuf::from(path)
    } else {
        locate()
            .await?
            .first()
            .map(|candidate| PathBuf::from(&candidate.path))
            .ok_or_else(|| AppError::new("wangshangliao_not_found", "没有找到旺商聊安装路径"))?
    };
    value
        .canonicalize()
        .map_err(|error| AppError::new("wangshangliao_path", format!("旺商聊路径不可用：{error}")))
}

#[cfg(windows)]
fn profile_script_path(image_path: &Path) -> PathBuf {
    image_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("resources")
        .join("app")
        .join("dist-electron")
        .join("main")
        .join("index.js")
}

#[cfg(windows)]
fn profile_backup_dir() -> AppResult<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("profile_path", "找不到 Windows APPDATA 目录"))?;
    Ok(appdata.join("DH").join("3.0").join("wangshangliao-backups"))
}

#[cfg(windows)]
fn latest_profile_backup() -> AppResult<Option<PathBuf>> {
    let directory = profile_backup_dir()?;
    if !directory.is_dir() {
        return Ok(None);
    }
    let mut files = std::fs::read_dir(&directory)
        .map_err(|error| AppError::new("profile_backup", error.to_string()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("index.js.") && name.ends_with(".bak"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files.pop())
}

#[cfg(windows)]
fn count_bytes(content: &[u8], marker: &[u8]) -> usize {
    content
        .windows(marker.len())
        .filter(|value| *value == marker)
        .count()
}

#[cfg(windows)]
fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

#[cfg(windows)]
fn atomic_replace(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("js.dh-restore-tmp");
    std::fs::write(&temporary, content)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn list_processes(image_path: &Path) -> AppResult<Vec<ProcessRef>> {
    use std::os::windows::process::CommandExt;
    let script = "$ErrorActionPreference='Stop'; Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -ne $null } | Select-Object ProcessId,ExecutablePath,CreationDate | ConvertTo-Json -Compress";
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000)
        .output()
        .map_err(|error| AppError::new("process_list", error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::new("process_list", "读取旺商聊进程失败"));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let values: Vec<serde_json::Value> = if raw.trim_start().starts_with('[') {
        serde_json::from_str(&raw).unwrap_or_default()
    } else if raw.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str::<serde_json::Value>(&raw)
            .map(|value| vec![value])
            .unwrap_or_default()
    };
    let want = image_path.to_string_lossy().to_lowercase();
    Ok(values
        .into_iter()
        .filter_map(|value| {
            let path = value.get("ExecutablePath")?.as_str()?.to_string();
            if path.to_lowercase() != want {
                return None;
            }
            Some(ProcessRef {
                pid: value.get("ProcessId")?.as_u64()? as u32,
                image_path: path,
                started_at: value
                    .get("CreationDate")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
                creation_time: value
                    .get("CreationDate")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
            })
        })
        .collect())
}

#[cfg(windows)]
fn stop_process(process: &ProcessRef) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    let current = list_processes(Path::new(&process.image_path))?;
    if !current.iter().any(|value| {
        value.pid == process.pid
            && value.image_path.eq_ignore_ascii_case(&process.image_path)
            && (process.creation_time.is_empty()
                || value.creation_time.is_empty()
                || value.creation_time == process.creation_time)
    }) {
        return Ok(());
    }
    let mut command = std::process::Command::new("taskkill");
    command
        .args(["/PID", &process.pid.to_string(), "/T"])
        .creation_flags(0x08000000);
    let _ = command.output();
    std::thread::sleep(Duration::from_secs(3));
    if list_processes(Path::new(&process.image_path))?
        .iter()
        .any(|value| value.pid == process.pid)
    {
        let mut force = std::process::Command::new("taskkill");
        force
            .args(["/PID", &process.pid.to_string(), "/T", "/F"])
            .creation_flags(0x08000000);
        let _ = force.output();
    }
    Ok(())
}

#[cfg(windows)]
fn activate(pid: u32) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    let script = format!(
        r#"$sig='[DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd,int nCmdShow);[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);';Add-Type -MemberDefinition $sig -Name Win -Namespace DH; $p=Get-Process -Id {pid}; if($p.MainWindowHandle -ne 0){{[DH.Win]::ShowWindowAsync($p.MainWindowHandle,9);[DH.Win]::SetForegroundWindow($p.MainWindowHandle)}}"#
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(0x08000000)
        .output()
        .map_err(|error| AppError::new("window_activate", error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::new("window_activate", "旺商聊窗口恢复失败"))
    }
}

#[cfg(windows)]
fn format_system_time(value: SystemTime) -> String {
    value
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_paths_are_case_insensitive_on_windows() {
        assert!(Path::new("C:/WangShangLiao.exe")
            .to_string_lossy()
            .to_lowercase()
            .ends_with("wangshangliao.exe"));
    }
}
