#[cfg(any(test, windows))]
use sha2::{Digest, Sha256};
#[cfg(any(test, windows))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::time::{Duration, SystemTime};
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

#[cfg(windows)]
use serde::Deserialize;
use serde::Serialize;

#[cfg(any(test, windows))]
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
pub struct WangProcessIdentity {
    pub pid: u32,
    pub image_path: String,
    pub started_at: String,
    pub creation_time: String,
    pub file_version: String,
    pub sha256: String,
}

pub type ProcessRef = WangProcessIdentity;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WangMaintenanceStatus {
    pub state: String,
    pub script_hash: String,
    pub backup_path: Option<String>,
    pub requires_elevation: bool,
    pub detail: String,
}

#[cfg(windows)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBackupManifest {
    image_path: String,
    script_path: String,
    image_version: String,
    original_hash: String,
    patched_hash: String,
    backup_path: String,
    created_at: String,
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
    let mut profile_detail = match prepare_persistent_profile(&image_path) {
        Ok(detail) => detail,
        Err(error) if error.code == "profile_permission" => {
            request_profile_elevation("apply", Some(image_path.to_string_lossy().into()))?;
            return Ok(WangStartResult {
                status: "maintenance-required".into(),
                detail: "旺商聊安装目录需要管理员权限，已请求同一个 DH-BOT 进程执行维护；维护完成后请再次启动旺商聊。".into(),
                needs_confirmation: false,
                process: None,
            });
        }
        Err(error) => return Err(error),
    };
    if let Ok(Some(detail)) = migrate_login_partition() {
        profile_detail.push_str(&detail);
    }
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
    let file_version = image_file_version(&image_path).unwrap_or_default();
    let image_sha256 = sha256_file(&image_path).unwrap_or_default();
    let created_at = format_system_time(SystemTime::now());
    let started = ProcessRef {
        pid: child.id(),
        image_path: image_path.to_string_lossy().into(),
        started_at: created_at.clone(),
        creation_time: created_at,
        file_version,
        sha256: image_sha256,
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
    if count_bytes(&original, b"persist:dh-primary") == 1
        && count_bytes(&original, b"persist:${Date.now()}") == 0
    {
        return Ok("固定登录分区 dh-primary 已启用。".into());
    }
    let patched = patch_profile_content(&original)?;
    let hash = sha256_hex(&original);
    let patched_hash = sha256_hex(&patched);
    let backup_dir = profile_backup_dir()?;
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
    let manifest = ProfileBackupManifest {
        image_path: image_path.to_string_lossy().into(),
        script_path: script_path.to_string_lossy().into(),
        image_version: image_file_version(image_path).unwrap_or_default(),
        original_hash: hash.clone(),
        patched_hash,
        backup_path: backup_path.to_string_lossy().into(),
        created_at: format_system_time(SystemTime::now()),
    };
    write_profile_manifest(&backup_dir, &manifest)?;
    atomic_replace(&script_path, &patched).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "profile_permission"
        } else {
            "profile_write"
        };
        AppError::new(code, format!("应用旺商聊固定分区维护失败：{error}"))
    })?;
    let verified = std::fs::read(&script_path)
        .map(|content| sha256_hex(&content))
        .map_err(|error| AppError::new("profile_verify", error.to_string()))?;
    if verified != manifest.patched_hash {
        let _ = atomic_replace(&script_path, &original);
        return Err(AppError::new(
            "profile_verify",
            "旺商聊启动脚本维护校验失败，已恢复原文件",
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
    let backup_path = latest_profile_manifest(&script_path)?
        .map(|manifest| manifest.backup_path)
        .or_else(|| {
            latest_profile_backup()
                .ok()
                .flatten()
                .map(|path| path.to_string_lossy().into())
        });
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
            request_profile_elevation("apply", path.clone())?;
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
    let manifest = latest_profile_manifest(&script_path)?;
    let backup = if let Some(manifest) = &manifest {
        PathBuf::from(&manifest.backup_path)
    } else {
        latest_profile_backup()?.ok_or_else(|| {
            AppError::new(
                "profile_backup_missing",
                "没有找到可恢复的旺商聊启动脚本备份",
            )
        })?
    };
    let original = std::fs::read(&backup).map_err(|error| {
        AppError::new(
            "profile_backup",
            format!("读取旺商聊启动脚本备份失败：{error}"),
        )
    })?;
    let hash_mismatch = manifest
        .as_ref()
        .map(|manifest| sha256_hex(&original) != manifest.original_hash)
        .unwrap_or(false);
    if count_bytes(&original, b"persist:${Date.now()}") != 1 || hash_mismatch {
        return Err(AppError::new(
            "profile_backup_invalid",
            "启动脚本备份结构校验失败，已保持当前文件",
        ));
    }
    if let Err(error) = atomic_replace(&script_path, &original) {
        if error.kind() == std::io::ErrorKind::PermissionDenied && !maintenance_arguments_present()
        {
            request_profile_elevation("restore", path.clone())?;
            let mut status = profile_status(path).await?;
            status.state = "elevation-requested".into();
            status.requires_elevation = true;
            status.detail = "已请求管理员权限恢复旺商聊原启动脚本。".into();
            return Ok(status);
        }
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "profile_permission"
        } else {
            "profile_restore"
        };
        return Err(AppError::new(
            code,
            format!("恢复旺商聊启动脚本失败：{error}"),
        ));
    }
    profile_status(path).await
}

#[cfg(windows)]
pub fn request_profile_elevation(operation: &str, path: Option<String>) -> AppResult<()> {
    if !matches!(operation, "apply" | "restore") {
        return Err(AppError::new("maintenance", "维护操作参数无效"));
    }
    let executable = std::env::current_exe()
        .map_err(|error| AppError::new("maintenance_uac", error.to_string()))?;
    let image = path.unwrap_or_default();
    let args = format!(
        "--maintenance {operation} --wang-path \"{}\"",
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
fn migrate_login_partition() -> AppResult<Option<String>> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("profile_path", "找不到 Windows APPDATA 目录"))?;
    let mut partition_roots = [
        appdata.join("wangshangliao").join("Partitions"),
        appdata.join("wangshangliao_win_online").join("Partitions"),
        appdata.join("WangShangLiao").join("Partitions"),
        appdata.join("旺商聊").join("Partitions"),
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .collect::<Vec<_>>();
    if let Ok(entries) = std::fs::read_dir(&appdata) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.contains("wangshangliao") && !name.contains("旺商聊") {
                continue;
            }
            let candidate = entry.path().join("Partitions");
            if candidate.is_dir() && !partition_roots.contains(&candidate) {
                partition_roots.push(candidate);
            }
        }
    }
    for root in partition_roots {
        let target = root.join("dh-primary");
        if target.is_dir() {
            continue;
        }
        let mut candidates = std::fs::read_dir(&root)
            .map_err(|error| AppError::new("profile_partition", error.to_string()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter(|entry| entry.file_name().to_string_lossy() != "dh-primary")
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((entry.path(), modified))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, modified)| *modified);
        let Some((source, _)) = candidates.pop() else {
            continue;
        };
        std::fs::rename(&source, &target).map_err(|error| {
            AppError::new(
                "profile_partition",
                format!("迁移旺商聊登录分区失败，原登录数据保持不变：{error}"),
            )
        })?;
        return Ok(Some(
            "已将最近使用的旺商聊登录分区迁移为 dh-primary。".into(),
        ));
    }
    Ok(None)
}

#[cfg(windows)]
fn write_profile_manifest(directory: &Path, manifest: &ProfileBackupManifest) -> AppResult<()> {
    let path = directory.join(format!("profile.{}.json", manifest.original_hash));
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_vec_pretty(manifest)
        .map_err(|error| AppError::new("profile_manifest", error.to_string()))?;
    std::fs::write(&temporary, content)
        .map_err(|error| AppError::new("profile_manifest", error.to_string()))?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| AppError::new("profile_manifest", error.to_string()))?;
    }
    std::fs::rename(&temporary, &path)
        .map_err(|error| AppError::new("profile_manifest", error.to_string()))?;
    Ok(())
}

#[cfg(windows)]
fn latest_profile_manifest(script_path: &Path) -> AppResult<Option<ProfileBackupManifest>> {
    let directory = profile_backup_dir()?;
    if !directory.is_dir() {
        return Ok(None);
    }
    let expected = script_path.to_string_lossy().to_lowercase();
    let mut manifests = std::fs::read_dir(&directory)
        .map_err(|error| AppError::new("profile_manifest", error.to_string()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("profile.") && name.ends_with(".json"))
                .unwrap_or(false)
        })
        .filter_map(|path| std::fs::read(path).ok())
        .filter_map(|content| serde_json::from_slice::<ProfileBackupManifest>(&content).ok())
        .filter(|manifest| manifest.script_path.to_lowercase() == expected)
        .collect::<Vec<_>>();
    manifests.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(manifests.pop())
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

#[cfg(any(test, windows))]
fn count_bytes(content: &[u8], marker: &[u8]) -> usize {
    content
        .windows(marker.len())
        .filter(|value| *value == marker)
        .count()
}

#[cfg(any(test, windows))]
fn patch_profile_content(original: &[u8]) -> AppResult<Vec<u8>> {
    const ORIGINAL: &[u8] = b"persist:${Date.now()}";
    const PATCHED: &[u8] = b"persist:dh-primary";
    if count_bytes(original, PATCHED) == 1 && count_bytes(original, ORIGINAL) == 0 {
        return Ok(original.to_vec());
    }
    if count_bytes(original, ORIGINAL) != 1 || count_bytes(original, PATCHED) != 0 {
        return Err(AppError::new(
            "profile_unknown_version",
            "旺商聊启动脚本版本与已知结构不一致，已保持原文件",
        ));
    }
    let index = original
        .windows(ORIGINAL.len())
        .position(|window| window == ORIGINAL)
        .expect("validated profile marker");
    let mut patched = Vec::with_capacity(original.len() - ORIGINAL.len() + PATCHED.len());
    patched.extend_from_slice(&original[..index]);
    patched.extend_from_slice(PATCHED);
    patched.extend_from_slice(&original[index + ORIGINAL.len()..]);
    Ok(patched)
}

#[cfg(any(test, windows))]
fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

#[cfg(windows)]
fn atomic_replace(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("js.dh-restore-tmp");
    std::fs::write(&temporary, content)?;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = wide(temporary.as_os_str());
    let destination = wide(path.as_os_str());
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        let error = std::io::Error::last_os_error();
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
    let file_version = image_file_version(image_path).unwrap_or_default();
    let image_sha256 = sha256_file(image_path).unwrap_or_default();
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
                file_version: file_version.clone(),
                sha256: image_sha256.clone(),
            })
        })
        .collect())
}

#[cfg(windows)]
fn stop_process(process: &ProcessRef) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    if process.creation_time.is_empty() || process.sha256.is_empty() {
        return Err(AppError::new(
            "process_identity_incomplete",
            "旺商聊进程身份信息不完整，已保持当前进程",
        ));
    }
    let current = list_processes(Path::new(&process.image_path))?;
    if !current.iter().any(|value| {
        value.pid == process.pid
            && value.image_path.eq_ignore_ascii_case(&process.image_path)
            && value.creation_time == process.creation_time
            && value.sha256.eq_ignore_ascii_case(&process.sha256)
            && (process.file_version.is_empty() || value.file_version == process.file_version)
    }) {
        return Err(AppError::new(
            "process_identity_changed",
            "旺商聊进程身份已变化，已保持当前进程",
        ));
    }
    let close_script = format!(
        "$p=Get-Process -Id {} -ErrorAction Stop; if($p.MainWindowHandle -ne 0){{$null=$p.CloseMainWindow()}}",
        process.pid
    );
    let mut command = std::process::Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &close_script,
        ])
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
fn sha256_file(path: &Path) -> AppResult<String> {
    let content = std::fs::read(path)
        .map_err(|error| AppError::new("process_hash", format!("读取程序文件失败：{error}")))?;
    Ok(sha256_hex(&content))
}

#[cfg(windows)]
fn image_file_version(path: &Path) -> AppResult<String> {
    use std::os::windows::process::CommandExt;
    let escaped = path.to_string_lossy().replace('\'', "''");
    let script =
        format!("(Get-Item -LiteralPath '{escaped}' -ErrorAction Stop).VersionInfo.FileVersion");
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
        .map_err(|error| AppError::new("process_version", error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::new("process_version", "读取旺商聊文件版本失败"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

    #[test]
    fn profile_patch_requires_one_known_marker_and_is_idempotent() {
        let original = b"before persist:${Date.now()} after";
        let patched = patch_profile_content(original).unwrap();
        assert_eq!(patched, b"before persist:dh-primary after");
        assert_eq!(patch_profile_content(&patched).unwrap(), patched);
        assert_eq!(
            patch_profile_content(b"persist:${Date.now()} persist:${Date.now()}")
                .unwrap_err()
                .code,
            "profile_unknown_version"
        );
    }

    #[test]
    fn profile_hash_is_stable() {
        assert_eq!(
            sha256_hex(b"DH BOT"),
            "60b71e1a28e87e89cdf3a3c71abb0e65ac87a20fe4743d3eff38b2d0de59a30a"
        );
    }
}
