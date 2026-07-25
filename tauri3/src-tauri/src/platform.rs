use sha2::{Digest, Sha256};
#[cfg(any(test, windows))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::time::{Duration, Instant, SystemTime};
#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::error::AppResult;
use crate::gateway::{CdpClient, ConnectionStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallCandidate {
    pub path: String,
    pub source: String,
    pub version: String,
    pub modified_at: String,
    pub running: bool,
    pub verified: bool,
    pub validation: String,
    pub priority: u32,
}

pub type DiscoveryProgress = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

#[cfg(windows)]
const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(300);

#[cfg(windows)]
static DISCOVERY_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedDiscovery>>> =
    std::sync::OnceLock::new();

#[cfg(windows)]
static DISCOVERY_RUN_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

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
    pub request_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct WangMaintenanceResult {
    pub request_id: String,
    pub operation: String,
    pub success: bool,
    pub error_code: String,
    pub message: String,
    pub completed_at: String,
}

#[cfg(any(test, windows))]
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
    pub maintenance_request_id: Option<String>,
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
    let candidates = [
        std::path::PathBuf::from("/Applications/旺商聊.app"),
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Applications/旺商聊.app"),
    ];
    Ok(candidates
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| InstallCandidate {
            path: path.display().to_string(),
            source: "macOS 常见安装目录".into(),
            version: String::new(),
            modified_at: String::new(),
            running: false,
            verified: true,
            validation: "应用包结构有效".into(),
            priority: 0,
        })
        .collect())
}

#[cfg(not(windows))]
pub async fn resolve_installation(
    saved_path: Option<String>,
    _progress: Option<DiscoveryProgress>,
) -> AppResult<InstallCandidate> {
    if let Some(path) = saved_path.filter(|value| !value.trim().is_empty()) {
        return Ok(InstallCandidate {
            path,
            source: "已保存路径".into(),
            version: String::new(),
            modified_at: String::new(),
            running: false,
            verified: true,
            validation: "应用包结构有效".into(),
            priority: 0,
        });
    }
    locate()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::new("wangshangliao_not_found", "没有找到旺商聊安装路径"))
}

#[cfg(not(windows))]
pub async fn running_process_identity_for(path: Option<String>) -> AppResult<Option<ProcessRef>> {
    let app = macos_app_root(path.as_deref());
    let executable = app.join("Contents").join("MacOS").join("旺商聊");
    let script = app
        .join("Contents")
        .join("Resources")
        .join("app")
        .join("dist-electron")
        .join("main")
        .join("index.js");
    if !executable.is_file() || !script.is_file() {
        return Ok(None);
    }
    let output = std::process::Command::new("pgrep")
        .args(["-f", executable.to_string_lossy().as_ref()])
        .output()
        .map_err(|error| AppError::new("process_inspect", error.to_string()))?;
    let pid = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if pid == 0 {
        return Ok(None);
    }
    let file_version = macos_bundle_version(&app);
    let script_hash = sha256_hex(
        &std::fs::read(&script)
            .map_err(|error| AppError::new("process_hash", error.to_string()))?,
    );
    Ok(Some(ProcessRef {
        pid,
        image_path: executable.display().to_string(),
        started_at: String::new(),
        creation_time: String::new(),
        file_version,
        sha256: script_hash,
    }))
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
        maintenance_request_id: None,
        process: None,
    })
}

#[cfg(not(windows))]
pub async fn focus(_path: Option<String>) -> AppResult<()> {
    Err(AppError::new(
        "wangshangliao_focus",
        "旺商聊窗口唤起仅在 Windows 可用。",
    ))
}

#[cfg(not(windows))]
pub async fn profile_status(path: Option<String>) -> AppResult<WangMaintenanceStatus> {
    let app = macos_app_root(path.as_deref());
    let script = app
        .join("Contents")
        .join("Resources")
        .join("app")
        .join("dist-electron")
        .join("main")
        .join("index.js");
    if script.is_file() {
        let content = std::fs::read(&script)
            .map_err(|error| AppError::new("profile_read", error.to_string()))?;
        return Ok(WangMaintenanceStatus {
            state: "manual-platform".into(),
            script_hash: sha256_hex(&content),
            backup_path: None,
            requires_elevation: false,
            request_id: None,
            detail: "macOS 仅读取旺商聊脚本指纹，不执行进程维护。".into(),
        });
    }
    Ok(WangMaintenanceStatus {
        state: "unsupported".into(),
        script_hash: String::new(),
        backup_path: None,
        requires_elevation: false,
        request_id: None,
        detail: "旺商聊启动脚本维护仅在 Windows 可用。".into(),
    })
}

#[cfg(not(windows))]
fn macos_app_root(path: Option<&str>) -> std::path::PathBuf {
    let value = path
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/Applications/旺商聊.app"));
    if value.extension().and_then(|ext| ext.to_str()) == Some("app") {
        value
    } else {
        value
            .ancestors()
            .find(|candidate| candidate.extension().and_then(|ext| ext.to_str()) == Some("app"))
            .map(std::path::Path::to_path_buf)
            .unwrap_or(value)
    }
}

#[cfg(not(windows))]
fn macos_bundle_version(app: &std::path::Path) -> String {
    std::process::Command::new("plutil")
        .args([
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-o",
            "-",
            app.join("Contents/Info.plist").to_string_lossy().as_ref(),
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
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

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn ensure_webview2_runtime() -> bool {
    true
}

#[cfg(not(windows))]
pub fn show_startup_error(message: &str) {
    eprintln!("{message}");
}

#[cfg(windows)]
pub fn ensure_webview2_runtime() -> bool {
    if webview2_runtime_paths().iter().any(|path| path.is_dir()) {
        return true;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title = wide("DH BOT 缺少 WebView2");
    let message = wide(
        "当前电脑没有检测到 Microsoft Edge WebView2 Runtime。\n\n请使用 DH BOT 安装版（已携带离线 WebView2），或先安装 WebView2 Runtime 后再打开便携版。",
    );
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    false
}

#[cfg(windows)]
pub fn show_startup_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title = wide("DH BOT 启动失败");
    let message = wide(message);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(windows)]
fn webview2_runtime_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in [
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("ProgramFiles"),
        std::env::var_os("LOCALAPPDATA"),
    ]
    .into_iter()
    .flatten()
    {
        paths.push(
            PathBuf::from(root)
                .join("Microsoft")
                .join("EdgeWebView")
                .join("Application"),
        );
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            paths.push(directory.join("WebView2FixedVersion"));
        }
    }
    paths
}

#[cfg(windows)]
pub async fn locate() -> AppResult<Vec<InstallCandidate>> {
    discover_installations(None, None).await
}

#[cfg(windows)]
pub async fn resolve_installation(
    saved_path: Option<String>,
    progress: Option<DiscoveryProgress>,
) -> AppResult<InstallCandidate> {
    let candidates = discover_installations(saved_path, progress).await?;
    candidates.into_iter().next().ok_or_else(|| {
        AppError::new(
            "not-found",
            "已检查运行进程、快捷方式、注册表、常见目录和本地固定磁盘，但没有找到验证通过的旺商聊程序。",
        )
    })
}

#[cfg(windows)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInstallCandidate {
    path: String,
    source: String,
}

#[cfg(windows)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImageVersionInfo {
    #[serde(default)]
    product_name: String,
    #[serde(default)]
    file_description: String,
    #[serde(default)]
    company_name: String,
    #[serde(default)]
    file_version: String,
}

#[cfg(windows)]
#[derive(Clone)]
struct CachedDiscovery {
    at: Instant,
    candidates: Vec<InstallCandidate>,
    permission_denied: bool,
}

#[cfg(windows)]
async fn discover_installations(
    saved_path: Option<String>,
    progress: Option<DiscoveryProgress>,
) -> AppResult<Vec<InstallCandidate>> {
    tokio::task::spawn_blocking(move || discover_installations_blocking(saved_path, progress))
        .await
        .map_err(|error| AppError::new("wangshangliao_discovery", error.to_string()))?
}

#[cfg(windows)]
fn discover_installations_blocking(
    saved_path: Option<String>,
    progress: Option<DiscoveryProgress>,
) -> AppResult<Vec<InstallCandidate>> {
    let _guard = DISCOVERY_RUN_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| AppError::new("wangshangliao_discovery", "旺商聊路径扫描锁已损坏"))?;
    report_discovery(
        &progress,
        "discovering",
        "正在快速检查旺商聊运行进程和已保存路径。",
    );

    let mut raw = discover_registered_installations()?;
    if let Some(path) = saved_path.filter(|value| !value.trim().is_empty()) {
        raw.push(RawInstallCandidate {
            path,
            source: "已保存路径".into(),
        });
    }
    raw.extend(common_installation_candidates());

    report_discovery(&progress, "validating", "正在验证找到的旺商聊程序。");
    let mut candidates = validate_candidates(raw);
    if candidates.is_empty() {
        let cache = DISCOVERY_CACHE.get_or_init(|| std::sync::Mutex::new(None));
        let cached = cache
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .filter(|value: &CachedDiscovery| value.at.elapsed() < DISCOVERY_CACHE_TTL);
        let discovery = if let Some(cached) = cached {
            report_discovery(
                &progress,
                "discovering",
                "正在使用最近一次固定磁盘扫描结果。",
            );
            cached
        } else {
            report_discovery(
                &progress,
                "discovering",
                "快速来源未命中，正在扫描本地固定磁盘。",
            );
            let (paths, permission_denied) = scan_fixed_drives();
            report_discovery(
                &progress,
                "validating",
                "正在验证磁盘扫描找到的旺商聊程序。",
            );
            let value = CachedDiscovery {
                at: Instant::now(),
                candidates: validate_candidates(paths),
                permission_denied,
            };
            if let Ok(mut cache) = cache.lock() {
                *cache = Some(value.clone());
            }
            value
        };
        candidates.extend(discovery.candidates);
        if candidates.is_empty() && discovery.permission_denied {
            return Err(AppError::new(
                "permission-denied",
                "部分本地目录没有读取权限，且其余可读取位置未找到旺商聊程序。",
            ));
        }
    }
    sort_and_deduplicate_candidates(&mut candidates);
    Ok(candidates)
}

#[cfg(windows)]
fn report_discovery(progress: &Option<DiscoveryProgress>, status: &str, detail: &str) {
    if let Some(progress) = progress {
        progress(status, detail);
    }
}

#[cfg(windows)]
fn common_installation_candidates() -> Vec<RawInstallCandidate> {
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
            candidates.push(RawInstallCandidate {
                path: PathBuf::from(&root).join(relative).to_string_lossy().into(),
                source: "常见安装目录".into(),
            });
        }
    }
    candidates
}

#[cfg(windows)]
fn validate_candidates(raw: Vec<RawInstallCandidate>) -> Vec<InstallCandidate> {
    let mut raw = raw;
    raw.sort_by(|left, right| {
        candidate_source_priority(&left.source)
            .cmp(&candidate_source_priority(&right.source))
            .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
    });
    raw.dedup_by(|left, right| left.path.eq_ignore_ascii_case(&right.path));
    raw.into_iter()
        .filter_map(|candidate| validate_candidate(Path::new(&candidate.path), &candidate.source))
        .collect()
}

#[cfg(windows)]
fn validate_candidate(path: &Path, source: &str) -> Option<InstallCandidate> {
    let canonical = path.canonicalize().ok()?;
    if !canonical.is_file() || !is_wang_executable_name(&canonical) {
        return None;
    }
    let directory = canonical.parent()?;
    if !["resources.pak", "icudtl.dat", "ffmpeg.dll"]
        .into_iter()
        .all(|name| directory.join(name).is_file())
    {
        return None;
    }
    let info = image_version_info(&canonical).ok()?;
    let product_identity =
        format!("{} {}", info.product_name, info.file_description).to_lowercase();
    let company_identity = info.company_name.to_lowercase();
    let matches_identity =
        |value: &str| value.contains("wangshangliao") || value.contains("旺商聊");
    if info.file_version.trim().is_empty()
        || !matches_identity(&product_identity)
        || !matches_identity(&company_identity)
    {
        return None;
    }
    let running = source == "正在运行";
    let modified_at = canonical
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map(format_system_time)
        .unwrap_or_default();
    Some(InstallCandidate {
        path: canonical.to_string_lossy().into(),
        source: source.into(),
        version: info.file_version,
        modified_at,
        running,
        verified: true,
        validation: "文件名、版本信息和 Electron 运行结构均有效".into(),
        priority: candidate_source_priority(source),
    })
}

#[cfg(windows)]
fn is_wang_executable_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    [
        "wangshangliao_win_online.exe",
        "wangshangliao.exe",
        "旺商聊.exe",
    ]
    .into_iter()
    .any(|expected| name.eq_ignore_ascii_case(expected))
}

#[cfg(windows)]
fn candidate_source_priority(source: &str) -> u32 {
    match source {
        "正在运行" => 0,
        "已保存路径" => 10,
        "用户桌面" => 20,
        "公共桌面" => 30,
        "开始菜单" => 40,
        "Windows 注册表" | "App Paths" => 50,
        "常见安装目录" => 60,
        "固定磁盘扫描" => 70,
        _ => 80,
    }
}

#[cfg(windows)]
fn sort_and_deduplicate_candidates(candidates: &mut Vec<InstallCandidate>) {
    candidates.sort_by(compare_install_candidates);
    let mut unique = Vec::<InstallCandidate>::new();
    for candidate in candidates.drain(..) {
        if unique
            .iter()
            .any(|value| windows_paths_equal(&value.path, &candidate.path))
        {
            continue;
        }
        unique.push(candidate);
    }
    *candidates = unique;
}

#[cfg(windows)]
fn compare_install_candidates(
    left: &InstallCandidate,
    right: &InstallCandidate,
) -> std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| compare_versions(&right.version, &left.version))
        .then_with(|| {
            right
                .modified_at
                .parse::<u64>()
                .unwrap_or_default()
                .cmp(&left.modified_at.parse::<u64>().unwrap_or_default())
        })
        .then_with(|| left.path.to_lowercase().cmp(&right.path.to_lowercase()))
}

#[cfg(any(test, windows))]
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let parts = |value: &str| {
        value
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse::<u64>().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let left = parts(left);
    let right = parts(right);
    for index in 0..left.len().max(right.len()) {
        let order = left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default());
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(any(test, windows))]
fn normalize_windows_path_for_compare(value: &str) -> String {
    let normalized = value.trim().replace('/', "\\").to_ascii_lowercase();
    let normalized = if let Some(path) = normalized.strip_prefix(r"\\?\unc\") {
        format!(r"\\{path}")
    } else if let Some(path) = normalized.strip_prefix(r"\\?\") {
        path.to_string()
    } else {
        normalized
    };
    normalized.trim_end_matches('\\').to_string()
}

#[cfg(any(test, windows))]
fn windows_paths_equal(left: &str, right: &str) -> bool {
    normalize_windows_path_for_compare(left) == normalize_windows_path_for_compare(right)
}

#[cfg(windows)]
fn select_connected_process(
    configured: Vec<ProcessRef>,
    discovered: Option<ProcessRef>,
) -> AppResult<Option<(ProcessRef, bool)>> {
    match configured.as_slice() {
        [] => Ok(discovered.map(|process| (process, true))),
        [process] => Ok(Some((process.clone(), false))),
        processes => Err(AppError::new(
            "devtools_process_ambiguous",
            format!(
                "检测到多个相同路径的旺商聊主进程（PID：{}），无法确认 9222 所属实例；请保留一个实例后重试，已保持所有进程不变",
                processes
                    .iter()
                    .map(|process| process.pid.to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        )),
    }
}

#[cfg(windows)]
pub async fn running_process_identity_for(path: Option<String>) -> AppResult<Option<ProcessRef>> {
    let mut processes = Vec::new();
    let paths = if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
        vec![PathBuf::from(path)]
    } else {
        locate()
            .await?
            .into_iter()
            .map(|candidate| PathBuf::from(candidate.path))
            .collect()
    };
    for path in paths {
        if !path.is_file() {
            continue;
        }
        processes.extend(list_processes(&path)?);
    }
    processes.sort_by_key(|process| process.pid);
    processes.dedup_by_key(|process| process.pid);
    Ok(match processes.as_slice() {
        [process] => Some(process.clone()),
        _ => None,
    })
}

#[cfg(windows)]
pub async fn start(
    path: Option<String>,
    devtools: String,
    confirm_restart: bool,
) -> AppResult<WangStartResult> {
    use std::os::windows::process::CommandExt;
    let image_path = if let Some(path) = path.filter(|value| !value.trim().is_empty()) {
        let candidate = validate_candidate(Path::new(&path), "已保存路径").ok_or_else(|| {
            AppError::new(
                "wangshangliao_path",
                "指定路径不是验证通过的旺商聊 Electron 主程序。",
            )
        })?;
        PathBuf::from(candidate.path)
    } else {
        PathBuf::from(resolve_installation(None, None).await?.path)
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
    if status == "other-service" {
        return Err(AppError::new(
            "devtools_port_occupied",
            "9222 端口已被其他程序的 DevTools 占用，已保持所有进程不变",
        ));
    }
    if matches!(
        status.as_str(),
        "ready" | "devtools-ready" | "nim-not-ready"
    ) {
        let configured_processes = list_processes(&image_path)?;
        let discovered_process = if configured_processes.is_empty() {
            running_process_identity_for(None).await.ok().flatten()
        } else {
            None
        };
        let Some((process, adopted_running_path)) =
            select_connected_process(configured_processes, discovered_process)?
        else {
            return Err(AppError::new(
                "devtools_process_mismatch",
                "9222 已连接旺商聊，但无法唯一确认对应主进程，已保持所有进程不变",
            ));
        };
        let image_path = if adopted_running_path {
            let running_path =
                PathBuf::from(&process.image_path)
                    .canonicalize()
                    .map_err(|error| {
                        AppError::new(
                            "devtools_process_mismatch",
                            format!("无法确认当前旺商聊主进程路径：{error}"),
                        )
                    })?;
            if validate_candidate(&running_path, "正在运行").is_none() {
                return Err(AppError::new(
                    "devtools_process_mismatch",
                    "9222 已连接旺商聊，但当前运行主进程未通过路径和程序校验，已保持所有进程不变",
                ));
            }
            running_path
        } else {
            image_path
        };
        let profile_detail = match prepare_persistent_profile(&image_path) {
            Ok(detail) => detail,
            Err(error) if error.code == "profile_permission" => {
                let request_id =
                    request_profile_elevation("apply", Some(image_path.to_string_lossy().into()))?;
                return Ok(WangStartResult {
                    status: "maintenance-required".into(),
                    detail: "旺商聊已连接；DH BOT 正在请求管理员权限启用账号登录复用，完成后会继续显示旺商聊。".into(),
                    needs_confirmation: false,
                    maintenance_request_id: Some(request_id),
                    process: None,
                });
            }
            Err(error) => format!("账号登录复用待处理：{}。", error.message),
        };
        let adopted_detail = if adopted_running_path {
            "已自动匹配正在运行的旺商聊版本。"
        } else {
            ""
        };
        activate(process.pid)?;
        return Ok(WangStartResult {
            status,
            detail: format!(
                "旺商聊已打开并恢复到前台。{adopted_detail}{profile_detail}如果 NIM 尚未就绪，请在旺商聊完成登录后稍候。"
            ),
            needs_confirmation: false,
            maintenance_request_id: None,
            process: Some(process),
        });
    }
    let processes = list_processes(&image_path)?;
    if !processes.is_empty() && !confirm_restart {
        return Ok(WangStartResult {
            status: "running-without-devtools".into(),
            detail: "检测到旺商聊已运行但没有 9222 DevTools，需要确认后重启。".into(),
            needs_confirmation: true,
            maintenance_request_id: None,
            process: processes.first().cloned(),
        });
    }
    let mut profile_detail = match prepare_persistent_profile(&image_path) {
        Ok(detail) => detail,
        Err(error) if error.code == "profile_permission" => {
            let request_id =
                request_profile_elevation("apply", Some(image_path.to_string_lossy().into()))?;
            return Ok(WangStartResult {
                status: "maintenance-required".into(),
                detail: "旺商聊安装目录需要管理员权限，已请求同一个 DH-BOT 进程执行维护；维护完成后请再次启动旺商聊。".into(),
                needs_confirmation: false,
                maintenance_request_id: Some(request_id),
                process: None,
            });
        }
        Err(error) => return Err(error),
    };
    if confirm_restart {
        for process in &processes {
            stop_process(process)?;
        }
    }
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
        let connection = inspect(&devtools)
            .await
            .unwrap_or_else(|_| "unavailable".into());
        if matches!(
            connection.as_str(),
            "ready" | "devtools-ready" | "nim-not-ready"
        ) {
            activate(started.pid)?;
            return Ok(WangStartResult {
                status: connection.clone(),
                detail: if connection == "ready" {
                    format!("旺商聊已启动、恢复到前台并完成会话初始化。{profile_detail}")
                } else {
                    format!(
                        "旺商聊已启动并恢复到前台。{profile_detail}请在显示的旺商聊窗口完成登录，DH BOT 会继续等待 NIM 初始化。"
                    )
                },
                needs_confirmation: false,
                maintenance_request_id: None,
                process: Some(started),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(WangStartResult {
        status: "started-waiting".into(),
        detail: format!("旺商聊进程已启动，但 DevTools 尚未响应。{profile_detail}"),
        needs_confirmation: false,
        maintenance_request_id: None,
        process: Some(started),
    })
}

#[cfg(windows)]
pub async fn focus(path: Option<String>) -> AppResult<()> {
    let image_path = resolve_image_path(path).await?;
    let processes = list_processes(&image_path)?;
    if let Some(process) = processes.first() {
        return activate(process.pid);
    }
    Err(AppError::new(
        "wangshangliao_not_running",
        "没有找到正在运行的旺商聊主进程。",
    ))
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
            request_id: None,
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
    let backup_path = latest_profile_manifest(&script_path)?.map(|manifest| manifest.backup_path);
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
        request_id: None,
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
            let request_id = request_profile_elevation("apply", path.clone())?;
            let mut status = profile_status(path).await?;
            status.state = "elevation-requested".into();
            status.requires_elevation = true;
            status.request_id = Some(request_id);
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
    let manifest = latest_profile_manifest(&script_path)?.ok_or_else(|| {
        AppError::new(
            "profile_backup_missing",
            "没有找到与当前旺商聊安装路径匹配的启动脚本备份",
        )
    })?;
    let backup = PathBuf::from(&manifest.backup_path);
    let original = std::fs::read(&backup).map_err(|error| {
        AppError::new(
            "profile_backup",
            format!("读取旺商聊启动脚本备份失败：{error}"),
        )
    })?;
    let hash_mismatch = sha256_hex(&original) != manifest.original_hash;
    if count_bytes(&original, b"persist:${Date.now()}") != 1 || hash_mismatch {
        return Err(AppError::new(
            "profile_backup_invalid",
            "启动脚本备份结构校验失败，已保持当前文件",
        ));
    }
    if let Err(error) = atomic_replace(&script_path, &original) {
        if error.kind() == std::io::ErrorKind::PermissionDenied && !maintenance_arguments_present()
        {
            let request_id = request_profile_elevation("restore", path.clone())?;
            let mut status = profile_status(path).await?;
            status.state = "elevation-requested".into();
            status.requires_elevation = true;
            status.request_id = Some(request_id);
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
pub fn request_profile_elevation(operation: &str, path: Option<String>) -> AppResult<String> {
    if !matches!(operation, "apply" | "restore") {
        return Err(AppError::new("maintenance", "维护操作参数无效"));
    }
    let executable = std::env::current_exe()
        .map_err(|error| AppError::new("maintenance_uac", error.to_string()))?;
    let image = path.unwrap_or_default();
    let request_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default()
    );
    let args = format!(
        "--maintenance {operation} --maintenance-request {request_id} --wang-path \"{}\"",
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
    Ok(request_id)
}

#[cfg(windows)]
pub fn run_maintenance_if_requested() -> bool {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--maintenance") {
        return false;
    }
    let operation = args.next().unwrap_or_default();
    let mut path = None;
    let mut request_id = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--wang-path" => path = args.next(),
            "--maintenance-request" => request_id = args.next(),
            _ => {}
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
    let maintenance_result = match result {
        Ok(status) => WangMaintenanceResult {
            request_id: request_id.clone().unwrap_or_default(),
            operation,
            success: true,
            error_code: String::new(),
            message: status.detail,
            completed_at: format_system_time(SystemTime::now()),
        },
        Err(error) => WangMaintenanceResult {
            request_id: request_id.clone().unwrap_or_default(),
            operation,
            success: false,
            error_code: error.code,
            message: error.message,
            completed_at: format_system_time(SystemTime::now()),
        },
    };
    if let Some(request_id) = request_id {
        let _ = write_maintenance_result(&request_id, &maintenance_result);
    }
    true
}

#[cfg(windows)]
pub fn maintenance_result(request_id: &str) -> AppResult<Option<WangMaintenanceResult>> {
    if request_id.is_empty()
        || !request_id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-')
    {
        return Err(AppError::new("maintenance_request", "维护请求编号无效"));
    }
    let path = maintenance_result_dir()?.join(format!("{request_id}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let value = std::fs::read(&path)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    let result = serde_json::from_slice(&value)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    let _ = std::fs::remove_file(path);
    Ok(Some(result))
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn maintenance_result(_request_id: &str) -> AppResult<Option<WangMaintenanceResult>> {
    Ok(None)
}

#[cfg(windows)]
fn maintenance_result_dir() -> AppResult<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("maintenance_result", "找不到 Windows APPDATA 目录"))?;
    Ok(appdata.join("DH").join("3.0").join("maintenance-results"))
}

#[cfg(windows)]
fn write_maintenance_result(request_id: &str, result: &WangMaintenanceResult) -> AppResult<()> {
    let directory = maintenance_result_dir()?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    let target = directory.join(format!("{request_id}.json"));
    let temporary = directory.join(format!("{request_id}.tmp"));
    let value = serde_json::to_vec(result)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    std::fs::write(&temporary, value)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    std::fs::rename(temporary, target)
        .map_err(|error| AppError::new("maintenance_result", error.to_string()))?;
    Ok(())
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
    let value = resolve_installation(path, None).await?;
    let value = PathBuf::from(value.path);
    value
        .canonicalize()
        .map_err(|error| AppError::new("wangshangliao_path", format!("旺商聊路径不可用：{error}")))
}

#[cfg(windows)]
fn discover_registered_installations() -> AppResult<Vec<RawInstallCandidate>> {
    use std::os::windows::process::CommandExt;
    let script = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'SilentlyContinue'
$items = New-Object System.Collections.Generic.List[object]
function Add-Candidate([string]$Path, [string]$Source) {
  if ([string]::IsNullOrWhiteSpace($Path)) { return }
  $value = [Environment]::ExpandEnvironmentVariables($Path.Trim().Trim('"'))
  $value = $value -replace ',\s*\d+$', ''
  $value = $value.Trim().Trim('"')
  if ((Test-Path -LiteralPath $value -PathType Leaf) -and ([IO.Path]::GetExtension($value) -eq '.exe')) {
    $items.Add([pscustomobject]@{ path = [IO.Path]::GetFullPath($value); source = $Source })
  }
}
$uninstallRoots = @(
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
foreach ($entry in Get-ItemProperty $uninstallRoots) {
  if ($entry.DisplayName -notmatch '旺商聊|wangshangliao') { continue }
  Add-Candidate $entry.DisplayIcon 'Windows 注册表'
  foreach ($name in @('wangshangliao_win_online.exe', 'WangShangLiao.exe', '旺商聊.exe')) {
    if ($entry.InstallLocation) { Add-Candidate (Join-Path $entry.InstallLocation $name) 'Windows 注册表' }
  }
}
$appPathRoots = @(
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths',
  'HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths'
)
foreach ($root in $appPathRoots) {
  foreach ($name in @('wangshangliao_win_online.exe', 'WangShangLiao.exe', '旺商聊.exe')) {
    $entry = Get-ItemProperty -LiteralPath (Join-Path $root $name)
    if ($entry.'(default)') { Add-Candidate $entry.'(default)' 'App Paths' }
    if ($entry.PSPath) {
      $defaultValue = (Get-Item -LiteralPath (Join-Path $root $name)).GetValue('')
      Add-Candidate $defaultValue 'App Paths'
    }
  }
}
$shell = New-Object -ComObject WScript.Shell
$menus = @(
  [pscustomobject]@{ Path=(Join-Path $env:USERPROFILE 'Desktop'); Source='用户桌面' },
  [pscustomobject]@{ Path=(Join-Path $env:PUBLIC 'Desktop'); Source='公共桌面' },
  [pscustomobject]@{ Path=(Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'); Source='开始菜单' },
  [pscustomobject]@{ Path=(Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs'); Source='开始菜单' }
)
foreach ($menu in $menus) {
  foreach ($shortcut in Get-ChildItem -LiteralPath $menu.Path -Filter '*.lnk' -Recurse) {
    if ($shortcut.BaseName -notmatch '旺商聊|wangshangliao') { continue }
    Add-Candidate ($shell.CreateShortcut($shortcut.FullName).TargetPath) $menu.Source
  }
}
$running = Get-CimInstance Win32_Process | Where-Object {
  $_.ExecutablePath -and
  $_.Name -match '^(wangshangliao_win_online|WangShangLiao|旺商聊)\.exe$' -and
  $_.CommandLine -notmatch '--type=|--utility-sub-type=|--crashpad-handler'
}
foreach ($process in $running) { Add-Candidate $process.ExecutablePath '正在运行' }
$items | ConvertTo-Json -Compress
"#;
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
        .map_err(|error| AppError::new("wangshangliao_discovery", error.to_string()))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(raw.trim()).unwrap_or_default();
    if value.is_array() {
        Ok(serde_json::from_value(value).unwrap_or_default())
    } else if value.is_object() {
        Ok(serde_json::from_value(value)
            .map(|candidate| vec![candidate])
            .unwrap_or_default())
    } else {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
fn scan_fixed_drives() -> (Vec<RawInstallCandidate>, bool) {
    let mut candidates = Vec::new();
    let mut permission_denied = false;
    for drive in fixed_drive_roots() {
        scan_directory(&drive, &mut candidates, &mut permission_denied);
    }
    (candidates, permission_denied)
}

#[cfg(windows)]
fn fixed_drive_roots() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED_TYPE: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|index| mask & (1 << index) != 0)
        .filter_map(|index| {
            let root = format!("{}:\\", (b'A' + index as u8) as char);
            let wide = wide(&root);
            (unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_FIXED_TYPE)
                .then(|| PathBuf::from(root))
        })
        .collect()
}

#[cfg(windows)]
fn scan_directory(
    root: &Path,
    candidates: &mut Vec<RawInstallCandidate>,
    permission_denied: &mut bool,
) {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                if directory == root && error.kind() == std::io::ErrorKind::PermissionDenied {
                    *permission_denied = true;
                }
                continue;
            }
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() || is_reparse_point(&path) {
                continue;
            }
            if file_type.is_dir() {
                if should_descend_into(&path) {
                    directories.push(path);
                }
                continue;
            }
            if file_type.is_file() && is_wang_executable_name(&path) {
                candidates.push(RawInstallCandidate {
                    path: path.to_string_lossy().into(),
                    source: "固定磁盘扫描".into(),
                });
            }
        }
    }
}

#[cfg(windows)]
fn is_reparse_point(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn should_descend_into(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    ![
        "$Recycle.Bin",
        "System Volume Information",
        "Recovery",
        "WindowsApps",
    ]
    .into_iter()
    .any(|blocked| name.eq_ignore_ascii_case(blocked))
}

#[cfg(windows)]
fn image_version_info(path: &Path) -> AppResult<ImageVersionInfo> {
    use std::os::windows::process::CommandExt;
    let escaped = path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $v=(Get-Item -LiteralPath '{escaped}' -ErrorAction Stop).VersionInfo; [pscustomobject]@{{ProductName=$v.ProductName;FileDescription=$v.FileDescription;CompanyName=$v.CompanyName;FileVersion=$v.FileVersion}} | ConvertTo-Json -Compress"
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
        .map_err(|error| AppError::new("process_version", error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::new(
            "process_version",
            "读取旺商聊文件版本信息失败",
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| AppError::new("process_version", error.to_string()))
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
            .filter(|entry| partition_has_session_data(&entry.path()))
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((entry.path(), modified))
            })
            .collect::<Vec<_>>();
        // A random Electron partition is moved only when there is one
        // unambiguous partition containing session storage. Multiple candidates
        // are left untouched so DH BOT never guesses which account to keep.
        if candidates.len() != 1 {
            continue;
        }
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
fn partition_has_session_data(path: &Path) -> bool {
    [
        path.join("Cookies"),
        path.join("Network").join("Cookies"),
        path.join("Local Storage").join("leveldb"),
        path.join("Session Storage"),
    ]
    .iter()
    .any(|candidate| candidate.exists())
}

#[cfg(windows)]
fn write_profile_manifest(directory: &Path, manifest: &ProfileBackupManifest) -> AppResult<()> {
    let path = directory.join(profile_manifest_name(manifest));
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

#[cfg(any(test, windows))]
fn profile_manifest_name(manifest: &ProfileBackupManifest) -> String {
    let path_hash = sha256_hex(manifest.script_path.to_lowercase().as_bytes());
    format!("profile.{path_hash}.{}.json", manifest.original_hash)
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
    let script = "$ErrorActionPreference='Stop'; Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -ne $null } | Select-Object ProcessId,ExecutablePath,CreationDate,CommandLine | ConvertTo-Json -Compress";
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
    let want = normalize_windows_path_for_compare(&image_path.to_string_lossy());
    let file_version = image_file_version(image_path).unwrap_or_default();
    let image_sha256 = sha256_file(image_path).unwrap_or_default();
    Ok(values
        .into_iter()
        .filter_map(|value| {
            let path = value.get("ExecutablePath")?.as_str()?.to_string();
            if normalize_windows_path_for_compare(&path) != want {
                return None;
            }
            if !is_main_wang_process_commandline(
                value
                    .get("CommandLine")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ) {
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

#[cfg(any(test, windows))]
fn is_main_wang_process_commandline(command_line: &str) -> bool {
    let command_line = command_line.to_ascii_lowercase();
    !command_line.contains("--type=")
        && !command_line.contains("--utility-sub-type=")
        && !command_line.contains("--crashpad-handler")
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
    let remaining = list_processes(Path::new(&process.image_path))?;
    if let Some(current) = remaining.iter().find(|value| value.pid == process.pid) {
        if current.creation_time != process.creation_time
            || !current.image_path.eq_ignore_ascii_case(&process.image_path)
            || !current.sha256.eq_ignore_ascii_case(&process.sha256)
            || (!process.file_version.is_empty() && current.file_version != process.file_version)
        {
            return Err(AppError::new(
                "process_identity_changed",
                "等待旺商聊正常退出时进程身份发生变化，已停止后续操作",
            ));
        }
        let mut force = std::process::Command::new("taskkill");
        force
            .args(["/PID", &process.pid.to_string(), "/T", "/F"])
            .creation_flags(0x08000000);
        let output = force
            .output()
            .map_err(|error| AppError::new("process_stop", error.to_string()))?;
        if !output.status.success() {
            return Err(AppError::new(
                "process_stop",
                format!(
                    "结束旺商聊进程失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
        if list_processes(Path::new(&process.image_path))?
            .iter()
            .any(|value| value.pid == process.pid && value.creation_time == process.creation_time)
        {
            return Err(AppError::new(
                "process_stop",
                "旺商聊进程仍在运行，已停止重启流程",
            ));
        }
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
    Ok(image_version_info(path)?.file_version)
}

#[cfg(windows)]
fn activate(pid: u32) -> AppResult<()> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId, SetForegroundWindow, ShowWindowAsync,
        SW_RESTORE,
    };

    struct WindowSearch {
        pid: u32,
        window: HWND,
    }

    unsafe extern "system" fn find_main_window(window: HWND, context: LPARAM) -> BOOL {
        let search = &mut *(context as *mut WindowSearch);
        let mut owner_pid = 0;
        GetWindowThreadProcessId(window, &mut owner_pid);
        if owner_pid != search.pid {
            return 1;
        }
        let mut class_name = [0u16; 256];
        let length = GetClassNameW(window, class_name.as_mut_ptr(), class_name.len() as i32);
        if length > 0
            && String::from_utf16_lossy(&class_name[..length as usize]) == "Chrome_WidgetWin_1"
        {
            search.window = window;
            return 0;
        }
        1
    }

    let mut search = WindowSearch {
        pid,
        window: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(
            Some(find_main_window),
            &mut search as *mut WindowSearch as LPARAM,
        );
    }
    if search.window.is_null() {
        return Err(AppError::new(
            "window_activate",
            "未找到旺商聊主窗口，进程仍在运行，请从旺商聊托盘图标打开",
        ));
    }
    unsafe {
        ShowWindowAsync(search.window, SW_RESTORE);
        if SetForegroundWindow(search.window) == 0 {
            return Err(AppError::new(
                "window_activate",
                "旺商聊主窗口已恢复，但 Windows 拒绝将它切换到前台",
            ));
        }
    }
    Ok(())
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

    #[cfg(windows)]
    fn candidate(path: &str, source: &str, version: &str, modified_at: &str) -> InstallCandidate {
        InstallCandidate {
            path: path.into(),
            source: source.into(),
            version: version.into(),
            modified_at: modified_at.into(),
            running: source == "正在运行",
            verified: true,
            validation: "validated".into(),
            priority: candidate_source_priority(source),
        }
    }

    #[cfg(windows)]
    fn process(pid: u32, path: &str) -> ProcessRef {
        ProcessRef {
            pid,
            image_path: path.into(),
            started_at: String::new(),
            creation_time: String::new(),
            file_version: "2.7.8".into(),
            sha256: "hash".into(),
        }
    }

    #[cfg(windows)]
    #[test]
    fn connected_process_prefers_configured_path() {
        let selected = process(10, "D:/wangshangliao.exe");
        let running = process(20, "C:/wangshangliao.exe");
        let result = select_connected_process(vec![selected.clone()], Some(running)).unwrap();
        assert_eq!(result.as_ref().map(|value| value.0.pid), Some(selected.pid));
        assert!(!result.expect("configured process").1);
    }

    #[cfg(windows)]
    #[test]
    fn connected_process_adopts_unique_running_path() {
        let running = process(20, "C:/wangshangliao.exe");
        let result = select_connected_process(Vec::new(), Some(running.clone())).unwrap();
        assert_eq!(result.as_ref().map(|value| value.0.pid), Some(running.pid));
        assert!(result.expect("running process").1);
    }

    #[cfg(windows)]
    #[test]
    fn connected_process_rejects_ambiguous_running_paths() {
        let error = select_connected_process(
            vec![
                process(20, "D:/wangshangliao.exe"),
                process(30, "D:/wangshangliao.exe"),
            ],
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "devtools_process_ambiguous");
        assert!(error.message.contains("20、30"));
    }

    #[test]
    fn process_paths_are_case_insensitive_on_windows() {
        assert!(windows_paths_equal(
            r"\\?\C:\Program Files\WangShangLiao.exe",
            r"c:/program files/wangshangliao.exe"
        ));
        assert!(windows_paths_equal(
            r"\\?\UNC\server\share\WangShangLiao.exe",
            r"\\server\share\wangshangliao.exe"
        ));
    }

    #[test]
    fn version_comparison_handles_numeric_segments() {
        assert_eq!(
            compare_versions("2.7.10", "2.7.8"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("3.0", "3.0.0"), std::cmp::Ordering::Equal);
    }

    #[cfg(windows)]
    #[test]
    fn installation_order_prefers_current_use_then_saved_then_desktop() {
        let mut candidates = vec![
            candidate("D:/registry.exe", "Windows 注册表", "9.0.0", "9"),
            candidate("D:/desktop.exe", "用户桌面", "1.0.0", "1"),
            candidate("D:/saved.exe", "已保存路径", "1.0.0", "1"),
            candidate("D:/running.exe", "正在运行", "1.0.0", "1"),
        ];
        sort_and_deduplicate_candidates(&mut candidates);
        assert_eq!(candidates[0].source, "正在运行");
        assert_eq!(candidates[1].source, "已保存路径");
        assert_eq!(candidates[2].source, "用户桌面");
    }

    #[cfg(windows)]
    #[test]
    fn installation_order_uses_version_and_modified_time_for_equal_sources() {
        let mut candidates = vec![
            candidate("D:/old.exe", "固定磁盘扫描", "2.7.8", "100"),
            candidate("D:/newer.exe", "固定磁盘扫描", "2.7.10", "50"),
            candidate("D:/newest.exe", "固定磁盘扫描", "2.7.10", "200"),
        ];
        sort_and_deduplicate_candidates(&mut candidates);
        assert_eq!(candidates[0].path, "D:/newest.exe");
        assert_eq!(candidates[1].path, "D:/newer.exe");
        assert_eq!(candidates[2].path, "D:/old.exe");
    }

    #[cfg(windows)]
    #[test]
    fn duplicate_installations_keep_the_highest_priority_source() {
        let mut candidates = vec![
            candidate("D:/Wang.exe", "Windows 注册表", "2.7.8", "1"),
            candidate("d:/wang.exe", "正在运行", "2.7.8", "1"),
        ];
        sort_and_deduplicate_candidates(&mut candidates);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source, "正在运行");
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
    fn profile_manifests_are_scoped_to_the_installation_path() {
        let base = ProfileBackupManifest {
            image_path: "C:/one/wangshangliao.exe".into(),
            script_path: "C:/one/resources/app/index.js".into(),
            image_version: "1".into(),
            original_hash: "a".repeat(64),
            patched_hash: "b".repeat(64),
            backup_path: "backup".into(),
            created_at: "1".into(),
        };
        let mut second = base.clone();
        second.script_path = "D:/two/resources/app/index.js".into();
        assert_ne!(profile_manifest_name(&base), profile_manifest_name(&second));
    }

    #[test]
    fn profile_hash_is_stable() {
        assert_eq!(
            sha256_hex(b"DH BOT"),
            "60b71e1a28e87e89cdf3a3c71abb0e65ac87a20fe4743d3eff38b2d0de59a30a"
        );
    }

    #[test]
    fn electron_child_processes_are_not_treated_as_the_main_window() {
        assert!(is_main_wang_process_commandline(
            r#"C:\Program Files\旺商聊\wangshangliao.exe --remote-debugging-port=9222"#
        ));
        assert!(!is_main_wang_process_commandline(
            r#"C:\Program Files\旺商聊\wangshangliao.exe --type=renderer --lang=zh-CN"#
        ));
        assert!(!is_main_wang_process_commandline(
            r#"C:\Program Files\旺商聊\wangshangliao.exe --type=utility --utility-sub-type=network.mojom.NetworkService"#
        ));
    }
}
