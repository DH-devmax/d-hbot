use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{Local, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::build_channel::BuildChannel;
use crate::database::DatabaseStatus;
use crate::error::{AppError, AppResult};
use crate::gateway::{DiagnosticSnapshot, GatewayCapabilities};
use crate::models::AuditEvent;
use crate::paths::AppPaths;
use crate::runtime_work::DispatchStatsSnapshot;

const LOG_RETENTION_DAYS: u64 = 30;
const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOG_STORAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_MESSAGE_CHARS: usize = 4096;
const MAX_SUPPORT_LOG_FILES: usize = 20;
const MAX_SUPPORT_LOG_BYTES_PER_FILE: usize = 1024 * 1024;
const MAX_SUPPORT_AUDITS: usize = 200;
const SUPPORT_BUNDLE_RETENTION_DAYS: u64 = 30;
const MAX_SUPPORT_BUNDLES: usize = 20;
const MAX_SUPPORT_BUNDLE_STORAGE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogRecord<'a> {
    format_version: u8,
    timestamp: String,
    timestamp_utc: String,
    session_id: &'a str,
    sequence: u64,
    level: &'a str,
    message: String,
}

#[derive(Clone)]
pub struct Logger {
    directory: PathBuf,
    session_id: Arc<String>,
    sequence: Arc<AtomicU64>,
    write_lock: Arc<Mutex<()>>,
}

impl Logger {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let logger = Self {
            directory: directory.into(),
            session_id: Arc::new(uuid::Uuid::new_v4().to_string()),
            sequence: Arc::new(AtomicU64::new(0)),
            write_lock: Arc::new(Mutex::new(())),
        };
        let _ = logger.maintain();
        logger
    }

    pub fn write(&self, level: &str, message: &str) {
        if fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let message = truncate_log_message(redact(message));
        let record = LogRecord {
            format_version: 1,
            timestamp: Local::now().to_rfc3339(),
            timestamp_utc: Utc::now().to_rfc3339(),
            session_id: self.session_id.as_str(),
            sequence,
            level,
            message,
        };
        let Ok(value) = serde_json::to_string(&record) else {
            return;
        };
        let mut line = value.into_bytes();
        line.push(b'\n');
        let Ok(path) = self.log_path(line.len() as u64) else {
            return;
        };
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        if file.write_all(&line).is_ok() && matches!(level, "ERROR" | "FATAL") {
            let _ = file.sync_data();
        }
        // The process can run for weeks. Re-check bounds periodically without
        // paying for a directory scan on every ordinary INFO record.
        if sequence.is_multiple_of(256) {
            let _ = self.maintain();
        }
    }

    fn log_path(&self, incoming_bytes: u64) -> std::io::Result<PathBuf> {
        let date = Local::now().format("%Y%m%d").to_string();
        for part in 0..10_000 {
            let name = if part == 0 {
                format!("dh-{date}.jsonl")
            } else {
                format!("dh-{date}-{part:03}.jsonl")
            };
            let path = self.directory.join(name);
            if !path.exists() {
                return Ok(path);
            }
            let size = fs::metadata(&path)?.len();
            if size == 0 || size.saturating_add(incoming_bytes) <= MAX_LOG_FILE_BYTES {
                return Ok(path);
            }
        }
        Err(std::io::Error::other("当天诊断日志分段数量超出上限"))
    }

    fn maintain(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let now = SystemTime::now();
        let retention = Duration::from_secs(LOG_RETENTION_DAYS * 24 * 60 * 60);
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let path = entry.path();
            if !is_log_file(&path) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let modified = metadata.modified().unwrap_or(now);
            if now.duration_since(modified).unwrap_or_default() > retention {
                let _ = fs::remove_file(path);
                continue;
            }
            files.push((path, modified, metadata.len()));
        }
        files.sort_by_key(|entry| Reverse(entry.1));
        let mut total = 0_u64;
        for (path, _, length) in files {
            total = total.saturating_add(length);
            if total > MAX_LOG_STORAGE_BYTES {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
}

pub fn install_panic_hook(directory: PathBuf) {
    std::panic::set_hook(Box::new(move |panic| {
        let logger = Logger::new(directory.clone());
        let location = panic
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".into());
        let message = panic
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        logger.write("FATAL", &format!("{location} {message}"));
    }));
}

pub fn redact(message: &str) -> String {
    let patterns = [
        r#"(?i)(authorization\s*[:=]\s*)([^\r\n]+)"#,
        r#"(?i)(bearer\s+)([a-z0-9._~+/-]+)"#,
        r#"(?i)(\"?(?:api[_-]?key|token|cookie|secret|password|passwd|access[_-]?token|refresh[_-]?token|session(?:[_-]?id)?|sid)\"?\s*[:=]\s*\"?)([^\"\s,;}]+)"#,
        r#"(?i)([?&](?:api[_-]?key|token|key|access_token|refresh_token|session|sid)=)([^&#\s]+)"#,
        r#"(?i)((?:message|text|content|body|rawMessage|raw_message|内容|消息内容)\s*[=:：]\s*\"?)([^\"\r\n,;}]+)"#,
        r#"(?i)(sk-)[a-z0-9_-]{12,}"#,
        r#"(?i)([A-Z]:\\Users\\)[^\\\s]+"#,
        r#"(?i)(/Users/)[^/\s]+"#,
    ];
    let mut result = message.to_string();
    if let Ok(regex) = Regex::new(
        r#"(?i)((?:\"|')?(?:message|text|content|body|rawMessage|raw_message|内容|消息内容)(?:\"|')?\s*[:=：]\s*\")([^\"\r\n]*)(\")"#,
    ) {
        result = regex.replace_all(&result, "${1}***${3}").into_owned();
    }
    for pattern in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            result = if pattern.contains("sk-") {
                regex.replace_all(&result, "sk-***").into_owned()
            } else {
                regex.replace_all(&result, "${1}***").into_owned()
            };
        }
    }
    result
}

fn truncate_log_message(value: String) -> String {
    if value.chars().count() <= MAX_LOG_MESSAGE_CHARS {
        return value;
    }
    value
        .chars()
        .take(MAX_LOG_MESSAGE_CHARS)
        .collect::<String>()
        + "…<truncated>"
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleResult {
    pub path: String,
    pub sha256: String,
    pub included_files: usize,
    pub generated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportAuditEvent {
    timestamp: String,
    event: String,
    level: String,
    actor: String,
    group: String,
    member: String,
    details: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportManifest {
    format_version: u8,
    generated_at: String,
    files: Vec<SupportManifestEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupportManifestEntry {
    path: String,
    bytes: usize,
    sha256: String,
}

/// Builds a locally stored, privacy-preserving diagnostic bundle. The archive
/// deliberately excludes SQLite, secrets, Electron data and raw group content.
pub fn create_support_bundle(
    paths: &AppPaths,
    database: DatabaseStatus,
    audits: Vec<AuditEvent>,
    diagnostic: DiagnosticSnapshot,
    capabilities: GatewayCapabilities,
    dispatch_stats: DispatchStatsSnapshot,
) -> AppResult<SupportBundleResult> {
    let generated_at = Utc::now().to_rfc3339();
    let aliases = SupportAliases::from_audits(&audits);
    let diagnostic_document =
        support_diagnostic_document(paths, database, diagnostic, capabilities, &generated_at, &dispatch_stats)?;
    let audit_document = serde_json::to_vec_pretty(
        &audits
            .iter()
            .take(MAX_SUPPORT_AUDITS)
            .map(|audit| aliases.sanitize_audit(audit))
            .collect::<Vec<_>>(),
    )
    .map_err(|error| AppError::new("support_bundle", format!("生成审计摘要失败：{error}")))?;

    let mut entries = vec![
        ("README.txt".to_string(), support_readme().into_bytes()),
        ("diagnostic.json".to_string(), diagnostic_document),
        ("audit-summary.json".to_string(), audit_document),
    ];
    entries.extend(collect_recent_logs(&paths.logs, &aliases)?);

    let mut manifest_entries = entries
        .iter()
        .map(|(path, bytes)| SupportManifestEntry {
            path: path.clone(),
            bytes: bytes.len(),
            sha256: sha256_bytes(bytes),
        })
        .collect::<Vec<_>>();
    let checksums = manifest_entries
        .iter()
        .map(|entry| format!("{}  {}", entry.sha256, entry.path))
        .collect::<Vec<_>>()
        .join("\n");
    entries.push((
        "SHA256SUMS.txt".to_string(),
        format!("{checksums}\n").into_bytes(),
    ));
    manifest_entries.push(SupportManifestEntry {
        path: "SHA256SUMS.txt".into(),
        bytes: checksums.len() + 1,
        sha256: sha256_bytes(format!("{checksums}\n").as_bytes()),
    });
    let manifest = serde_json::to_vec_pretty(&SupportManifest {
        format_version: 1,
        generated_at: generated_at.clone(),
        files: manifest_entries,
    })
    .map_err(|error| AppError::new("support_bundle", format!("生成诊断清单失败：{error}")))?;
    entries.push(("manifest.json".to_string(), manifest));

    let directory = paths.v3.join("support-bundles");
    fs::create_dir_all(&directory)
        .map_err(|error| AppError::new("support_bundle", format!("创建诊断包目录失败：{error}")))?;
    maintain_support_bundles(&directory)?;
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let (output, temporary) = next_support_bundle_paths(&directory, &stamp)?;
    let result = write_zip(&temporary, &entries).and_then(|_| {
        fs::rename(&temporary, &output)
            .map_err(|error| AppError::new("support_bundle", format!("提交诊断包失败：{error}")))
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    maintain_support_bundles(&directory)?;
    let bytes = fs::read(&output)
        .map_err(|error| AppError::new("support_bundle", format!("校验诊断包失败：{error}")))?;
    Ok(SupportBundleResult {
        path: output.display().to_string(),
        sha256: sha256_bytes(&bytes),
        included_files: entries.len(),
        generated_at,
    })
}

fn write_zip(output: &Path, entries: &[(String, Vec<u8>)]) -> AppResult<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| AppError::new("support_bundle", format!("写入诊断包失败：{error}")))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(path, options).map_err(|error| {
            AppError::new("support_bundle", format!("写入诊断文件失败：{error}"))
        })?;
        zip.write_all(bytes).map_err(|error| {
            AppError::new("support_bundle", format!("写入诊断内容失败：{error}"))
        })?;
    }
    let file = zip
        .finish()
        .map_err(|error| AppError::new("support_bundle", format!("完成诊断包失败：{error}")))?;
    file.sync_all()
        .map_err(|error| AppError::new("support_bundle", format!("刷新诊断包失败：{error}")))?;
    Ok(())
}

fn next_support_bundle_paths(directory: &Path, stamp: &str) -> AppResult<(PathBuf, PathBuf)> {
    for part in 0..10_000 {
        let suffix = if part == 0 {
            String::new()
        } else {
            format!("-{part:03}")
        };
        let output = directory.join(format!("DH-BOT-support-{stamp}{suffix}.zip"));
        let temporary = directory.join(format!(".DH-BOT-support-{stamp}{suffix}.zip.tmp"));
        if !output.exists() && !temporary.exists() {
            return Ok((output, temporary));
        }
    }
    Err(AppError::new(
        "support_bundle",
        "当前秒内生成的诊断包数量超出上限，请稍后再试",
    ))
}

fn maintain_support_bundles(directory: &Path) -> AppResult<()> {
    let now = SystemTime::now();
    let retention = Duration::from_secs(SUPPORT_BUNDLE_RETENTION_DAYS * 24 * 60 * 60);
    let mut bundles = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| AppError::new("support_bundle", format!("读取诊断包目录失败：{error}")))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let modified = metadata.modified().unwrap_or(now);
        if name.starts_with(".DH-BOT-support-") && name.ends_with(".tmp") {
            if now.duration_since(modified).unwrap_or_default() > Duration::from_secs(60 * 60) {
                let _ = fs::remove_file(path);
            }
            continue;
        }
        if !name.starts_with("DH-BOT-support-") || !name.ends_with(".zip") {
            continue;
        }
        if now.duration_since(modified).unwrap_or_default() > retention {
            let _ = fs::remove_file(path);
            continue;
        }
        bundles.push((path, modified, metadata.len()));
    }
    bundles.sort_by_key(|entry| Reverse(entry.1));
    let mut total = 0_u64;
    for (index, (path, _, bytes)) in bundles.into_iter().enumerate() {
        total = total.saturating_add(bytes);
        if index >= MAX_SUPPORT_BUNDLES || total > MAX_SUPPORT_BUNDLE_STORAGE_BYTES {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn support_diagnostic_document(
    paths: &AppPaths,
    database: DatabaseStatus,
    diagnostic: DiagnosticSnapshot,
    capabilities: GatewayCapabilities,
    generated_at: &str,
    dispatch_stats: &DispatchStatsSnapshot,
) -> AppResult<Vec<u8>> {
    let mut capabilities = serde_json::to_value(capabilities)
        .map_err(|error| AppError::new("support_bundle", format!("序列化协议能力失败：{error}")))?;
    sanitize_json_value(&mut capabilities, None);
    let document = json!({
        "formatVersion": 1,
        "generatedAt": generated_at,
        "application": {
            "name": "DH BOT",
            "version": env!("CARGO_PKG_VERSION"),
            "buildChannel": BuildChannel::CURRENT.name(),
            "runtimeMode": paths.runtime_mode(),
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
        },
        "database": {
            "schemaVersion": database.schema_version,
            "integrity": database.integrity,
            "indexIntegrity": database.index_integrity,
            "accounts": database.accounts,
            "groups": database.groups,
            "messages": database.messages,
            "databaseIncluded": false,
        },
        "queueDepth": database.queue_depth,
        "retentionPolicies": database.retention_policies,
        "memoryCaps": {
            "maxMemberEventCache": 500,
            "maxReportedSet": 500,
            "maxMemberCacheGroups": 100,
            "maxTrackedWorkItems": 200,
            "note": "in-memory caps; actual values reflect compile-time constants",
        },
        "connection": {
            "status": diagnostic.status,
            "devtools": "loopback-only",
            "pageTitle": redact(&diagnostic.page_title),
            "pageUrl": "<local-page-omitted>",
            "nimAccount": "<account-omitted>",
            "detail": redact(&diagnostic.detail),
            "rateLimitHits": diagnostic.rate_limit_hits,
        },
        "dispatchStats": {
            "dispatched": dispatch_stats.dispatched,
            "proceeded": dispatch_stats.proceeded,
            "skipped": dispatch_stats.skipped,
            "rejected": dispatch_stats.rejected,
            "chainMicros": dispatch_stats.chain_micros,
        },
        "capabilities": capabilities,
        "privacy": {
            "database": "excluded",
            "secrets": "excluded",
            "wangshangliaoLoginData": "excluded",
            "rawMessages": "excluded",
            "identifiers": "aliased in audit summary",
        },
    });
    serde_json::to_vec_pretty(&document)
        .map_err(|error| AppError::new("support_bundle", format!("生成诊断信息失败：{error}")))
}

fn sanitize_json_value(value: &mut Value, key: Option<&str>) {
    match value {
        Value::String(text) => {
            let normalized_key = key.unwrap_or_default().to_ascii_lowercase();
            if matches!(normalized_key.as_str(), "nimaccount" | "pageurl") {
                *text = "<omitted>".into();
            } else if normalized_key == "fingerprint" && text.len() > 16 {
                *text = format!("{}...", &text[..16]);
            } else {
                *text = redact(text);
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|item| sanitize_json_value(item, None)),
        Value::Object(values) => values
            .iter_mut()
            .for_each(|(name, item)| sanitize_json_value(item, Some(name))),
        _ => {}
    }
}

fn collect_recent_logs(
    directory: &Path,
    aliases: &SupportAliases,
) -> AppResult<Vec<(String, Vec<u8>)>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(directory)
        .map_err(|error| AppError::new("support_bundle", format!("读取日志目录失败：{error}")))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !is_log_file(&path) {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| Reverse(entry.1));
    let mut entries = Vec::new();
    for (path, _) in files.into_iter().take(MAX_SUPPORT_LOG_FILES) {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let content = read_tail(&path, MAX_SUPPORT_LOG_BYTES_PER_FILE)?;
        let sanitized = content
            .lines()
            .map(|line| aliases.sanitize_log_line(line))
            .collect::<Vec<_>>()
            .join("\n");
        entries.push((
            format!("logs/{name}"),
            format!("{sanitized}\n").into_bytes(),
        ));
    }
    Ok(entries)
}

fn read_tail(path: &Path, limit: usize) -> AppResult<String> {
    let mut file = File::open(path)
        .map_err(|error| AppError::new("support_bundle", format!("读取日志文件失败：{error}")))?;
    let length = file
        .metadata()
        .map_err(|error| AppError::new("support_bundle", format!("读取日志大小失败：{error}")))?
        .len() as usize;
    if length > limit {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::End(-(limit as i64)))
            .map_err(|error| {
                AppError::new("support_bundle", format!("读取日志尾部失败：{error}"))
            })?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AppError::new("support_bundle", format!("读取日志内容失败：{error}")))?;
    if length > limit {
        // A byte tail can begin halfway through a JSONL record. Drop that
        // partial first line so every line in a support bundle is parseable.
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes = bytes.split_off(newline + 1);
        } else {
            bytes.clear();
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn is_log_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    name.starts_with("dh-") && (name.ends_with(".log") || name.ends_with(".jsonl"))
}

fn support_readme() -> String {
    [
        "DH BOT 诊断支持包",
        "此压缩包由使用者在本机手动生成，用于排查连接、协议、规则、AI 或任务异常。",
        "包含脱敏健康状态、协议能力、匿名化审计、近期脱敏日志和 SHA-256 校验文件。",
        "不包含 dh.db、secrets.dat、旺商聊登录数据、Cookie、API Key、Token、原始消息或源码。",
        "请把完整 ZIP、问题发生时间、操作步骤和截图一并发送给维护人员。",
        "请勿修改 ZIP 内文件；维护人员可使用 SHA256SUMS.txt 核对每个诊断文件。",
    ]
    .join("\n")
        + "\n"
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[derive(Default)]
struct SupportAliases {
    account: BTreeMap<String, String>,
    group: BTreeMap<i64, String>,
    member: BTreeMap<i64, String>,
    actor: BTreeMap<String, String>,
}

impl SupportAliases {
    fn from_audits(audits: &[AuditEvent]) -> Self {
        let mut aliases = Self::default();
        for audit in audits {
            aliases.account_alias(&audit.account_id);
            if audit.group_id > 0 {
                aliases.group_alias(audit.group_id);
            }
            if audit.user_id > 0 {
                aliases.member_alias(audit.user_id);
            }
            aliases.actor_alias(&audit.actor);
        }
        aliases
    }

    fn account_alias(&mut self, value: &str) -> String {
        if let Some(alias) = self.account.get(value) {
            return alias.clone();
        }
        let alias = format!("ACCOUNT-{:03}", self.account.len() + 1);
        self.account.insert(value.into(), alias.clone());
        alias
    }

    fn group_alias(&mut self, value: i64) -> String {
        if let Some(alias) = self.group.get(&value) {
            return alias.clone();
        }
        let alias = format!("GROUP-{:03}", self.group.len() + 1);
        self.group.insert(value, alias.clone());
        alias
    }

    fn member_alias(&mut self, value: i64) -> String {
        if let Some(alias) = self.member.get(&value) {
            return alias.clone();
        }
        let alias = format!("MEMBER-{:03}", self.member.len() + 1);
        self.member.insert(value, alias.clone());
        alias
    }

    fn actor_alias(&mut self, value: &str) -> String {
        if value.eq_ignore_ascii_case("DH BOT") || value.eq_ignore_ascii_case("system") {
            return value.into();
        }
        if let Some(alias) = self.actor.get(value) {
            return alias.clone();
        }
        let alias = format!("ACTOR-{:03}", self.actor.len() + 1);
        self.actor.insert(value.into(), alias.clone());
        alias
    }

    fn sanitize_audit(&self, audit: &AuditEvent) -> SupportAuditEvent {
        SupportAuditEvent {
            timestamp: audit.created_at.to_rfc3339(),
            event: audit.event.clone(),
            level: audit.level.clone(),
            actor: self.actor.get(&audit.actor).cloned().unwrap_or_else(|| {
                if audit.actor.eq_ignore_ascii_case("DH BOT")
                    || audit.actor.eq_ignore_ascii_case("system")
                {
                    audit.actor.clone()
                } else {
                    "ACTOR-UNKNOWN".into()
                }
            }),
            group: if audit.group_id > 0 {
                self.group
                    .get(&audit.group_id)
                    .cloned()
                    .unwrap_or_else(|| "GROUP-UNKNOWN".into())
            } else {
                "GLOBAL".into()
            },
            member: if audit.user_id > 0 {
                self.member
                    .get(&audit.user_id)
                    .cloned()
                    .unwrap_or_else(|| "MEMBER-UNKNOWN".into())
            } else {
                "NONE".into()
            },
            details: self.sanitize_text(&audit.details),
        }
    }

    fn sanitize_log_line(&self, value: &str) -> String {
        self.sanitize_text(value)
    }

    fn sanitize_text(&self, value: &str) -> String {
        let mut output = redact(value);
        for (raw, alias) in &self.account {
            if raw.len() >= 4 {
                output = output.replace(raw, alias);
            }
        }
        for (raw, alias) in &self.group {
            let text = raw.to_string();
            if text.len() >= 4 {
                output = output.replace(&text, alias);
            }
        }
        for (raw, alias) in &self.member {
            let text = raw.to_string();
            if text.len() >= 4 {
                output = output.replace(&text, alias);
            }
        }
        for pattern in [
            r#"(?i)((?:message|text|content|body|内容|消息内容)\s*[=:：]\s*)([^,;，；\n]+)"#,
            r#"(?i)((?:groupName|memberName|nickname|群名|成员名称|昵称)\s*[=:：]\s*)([^,;，；\n]+)"#,
            r#"(?i)((?:account|accountId|account_id|group|groupId|group_id|user|userId|user_id|nimId|nim_id|memberId|member_id|messageId|message_id|serverMessageId|server_message_id|eventId|event_id)\s*[=:：]\s*\"?)([^\"\s,;}]+)"#,
        ] {
            if let Ok(regex) = Regex::new(pattern) {
                output = regex.replace_all(&output, "${1}<omitted>").into_owned();
            }
        }
        if output.chars().count() > 800 {
            output = output.chars().take(800).collect::<String>() + "…<truncated>";
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{CapabilitySource, CapabilityStatus, GatewayCapability};
    use tempfile::tempdir;

    fn capabilities() -> GatewayCapabilities {
        let capability = GatewayCapability::new(
            CapabilityStatus::Supported,
            CapabilitySource::NimRuntime,
            true,
            true,
            "ready",
            "f".repeat(64),
        );
        GatewayCapabilities {
            announcement: capability.clone(),
            send_text: capability.clone(),
            mute: capability.clone(),
            recall: capability.clone(),
            rename: capability.clone(),
            remove_member: capability.clone(),
            group_mute: capability.clone(),
            member_events: capability,
        }
    }

    #[test]
    fn removes_common_secret_shapes() {
        let key = ["sk", "-", "abcdefghijklmnopqrstuvwxyz"].concat();
        let value = redact(
            &[
                "Author",
                "ization: Bearer ",
                "TOKEN apiKey=",
                &key,
                " cookie=session123",
            ]
            .concat(),
        );
        assert!(!value.contains("TOKEN"));
        assert!(!value.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!value.contains("session123"));
    }

    #[test]
    fn redacts_json_content_with_spaces() {
        let value = redact(r#"{"content":"private message with spaces","msg":"keep diagnostic"}"#);
        assert!(!value.contains("private message with spaces"));
        assert!(value.contains("content"));
        assert!(value.contains("***"));
    }

    #[test]
    fn redacts_query_credentials_and_user_paths() {
        let value = redact(
            "https://example.test/v1?token=abc123&x=1 /Users/alice/Documents C:\\Users\\alice\\DH",
        );
        assert!(!value.contains("abc123"));
        assert!(!value.contains("alice"));
        assert!(value.contains("***"));
    }

    #[test]
    fn logger_writes_ordered_dual_timestamps() {
        let directory = tempdir().unwrap();
        let logger = Logger::new(directory.path());
        logger.write("INFO", "connection ready");
        logger.write("ERROR", "request failed");
        let path = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| is_log_file(path))
            .unwrap();
        let records = fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["sequence"], 1);
        assert_eq!(records[1]["sequence"], 2);
        assert!(records[0]["timestamp"].as_str().unwrap().contains('T'));
        assert!(
            chrono::DateTime::parse_from_rfc3339(records[0]["timestampUtc"].as_str().unwrap())
                .is_ok()
        );
        assert!(!records[0]["sessionId"].as_str().unwrap().is_empty());
    }

    #[test]
    fn logger_truncates_large_messages_before_rotation() {
        let directory = tempdir().unwrap();
        let logger = Logger::new(directory.path());
        logger.write("INFO", &"x".repeat(MAX_LOG_MESSAGE_CHARS + 500));
        let path = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| is_log_file(path))
            .unwrap();
        let line = fs::read_to_string(path).unwrap();
        let record: Value = serde_json::from_str(line.trim()).unwrap();
        let message = record["message"].as_str().unwrap();
        assert!(message.ends_with("…<truncated>"));
        assert!(message.chars().count() <= MAX_LOG_MESSAGE_CHARS + 12);
    }

    #[test]
    fn support_log_tail_starts_at_complete_jsonl_record() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("dh-20260730.jsonl");
        fs::write(
            &path,
            "{\"sequence\":1,\"message\":\"first record\"}\n{\"sequence\":2,\"message\":\"second record\"}\n",
        )
        .unwrap();
        let tail = read_tail(&path, 52).unwrap();
        let lines = tail.lines().collect::<Vec<_>>();
        assert!(!lines.is_empty());
        assert!(lines
            .iter()
            .all(|line| serde_json::from_str::<Value>(line).is_ok()));
        assert!(!tail.starts_with("{\"sequence\":1"));
    }

    #[test]
    fn support_bundle_excludes_identifiers_and_secrets() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().join("DH"),
            v3: directory.path().join("DH").join("3.0"),
            database: directory.path().join("DH").join("3.0").join("dh.db"),
            secrets: directory.path().join("DH").join("3.0").join("secrets.dat"),
            logs: directory.path().join("DH").join("3.0").join("logs"),
            legacy_backups: directory.path().join("DH").join("legacy-backups"),
            runtime_mode_file: directory.path().join("DH").join("runtime-mode"),
        };
        fs::create_dir_all(&paths.logs).unwrap();
        fs::write(
            paths.logs.join("dh-20260730.log"),
            "token=top-secret account=998877 group=445566 content=private text account=111222\n",
        )
        .unwrap();
        let audit = AuditEvent {
            id: 1,
            account_id: "998877".into(),
            group_id: 445566,
            user_id: 223344,
            actor: "DH BOT".into(),
            event: "effect_dispatched".into(),
            level: "error".into(),
            details: "content=private text token=top-secret".into(),
            created_at: Utc::now(),
        };
        let result = create_support_bundle(
            &paths,
            DatabaseStatus {
                path: paths.database.display().to_string(),
                schema_version: 10,
                integrity: "ok".into(),
                accounts: 1,
                groups: 1,
                messages: 1,
                queue_depth: vec![],
                retention_policies: vec![],
                index_integrity: "ok".into(),
            },
            vec![audit.clone()],
            DiagnosticSnapshot {
                status: crate::gateway::ConnectionStatus::Ready,
                devtools_url: "http://127.0.0.1:9222".into(),
                page_title: "Wang".into(),
                page_url: "app://wang?token=top-secret".into(),
                nim_account: "998877".into(),
                detail: "ok".into(),
                rate_limit_hits: 0,
            },
            capabilities(),
            DispatchStatsSnapshot { dispatched: 0, proceeded: 0, skipped: 0, rejected: 0, chain_micros: 0 },
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&result.path).unwrap()).unwrap();
        let mut content = String::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            if entry.is_file() {
                let mut text = String::new();
                let _ = entry.read_to_string(&mut text);
                content.push_str(&text);
            }
        }
        assert!(!content.contains("top-secret"));
        assert!(!content.contains("998877"));
        assert!(!content.contains("445566"));
        assert!(!content.contains("223344"));
        assert!(!content.contains("111222"));
        assert!(!content.contains("private text"));
        assert!(content.contains("GROUP-001"));
        assert!(content.contains("MEMBER-001"));
        assert!(content.contains("SHA256SUMS.txt"));

        let mut checksums = String::new();
        archive
            .by_name("SHA256SUMS.txt")
            .unwrap()
            .read_to_string(&mut checksums)
            .unwrap();
        for line in checksums.lines() {
            let (expected, path) = line.split_once("  ").unwrap();
            let mut payload = Vec::new();
            archive
                .by_name(path)
                .unwrap()
                .read_to_end(&mut payload)
                .unwrap();
            assert_eq!(expected, sha256_bytes(&payload));
        }

        let repeated = create_support_bundle(
            &paths,
            DatabaseStatus {
                path: paths.database.display().to_string(),
                schema_version: 10,
                integrity: "ok".into(),
                accounts: 1,
                groups: 1,
                messages: 1,
                queue_depth: vec![],
                retention_policies: vec![],
                index_integrity: "ok".into(),
            },
            vec![audit],
            DiagnosticSnapshot {
                status: crate::gateway::ConnectionStatus::Ready,
                devtools_url: "http://127.0.0.1:9222".into(),
                page_title: "Wang".into(),
                page_url: "app://wang".into(),
                nim_account: "998877".into(),
                detail: "ok".into(),
                rate_limit_hits: 0,
            },
            capabilities(),
            DispatchStatsSnapshot { dispatched: 0, proceeded: 0, skipped: 0, rejected: 0, chain_micros: 0 },
        )
        .unwrap();
        assert_ne!(result.path, repeated.path);
        assert!(std::path::Path::new(&repeated.path).is_file());

        let bundle_dir = paths.v3.join("support-bundles");
        let mut bundle_paths = fs::read_dir(bundle_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("zip"))
            .collect::<Vec<_>>();
        bundle_paths.sort();
        assert_eq!(bundle_paths.len(), 2);
    }
}
