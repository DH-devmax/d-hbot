use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::defaults;
use crate::error::{AppError, AppResult, InternalError};
use crate::models::{
    Account, ActionRecord, AiProviderEndpoint, AuditEvent, BatchIngestResult, BusinessAppRecord,
    BusinessAppRun, CardPlan, CardRenameJob, DailySummary, EffectOutboxItem, EffectOutboxRequest,
    EnqueuedEffect, GatewayInboxEvent, GatewayInboxItem, Group, GroupAiPermissions, GroupSchedule,
    KnowledgeBase, KnowledgeDocument, Member, Message, ModerationRule, PersistedMessage, TaskItem,
};
use crate::paths::AppPaths;

pub struct MessageProcessingContext {
    pub group: Group,
    pub member: Option<Member>,
    pub recent_messages: Vec<Message>,
    pub rules: Vec<ModerationRule>,
}

pub struct RuntimeBacklog {
    pub account_id: String,
    pub group_id: i64,
    pub lane: String,
    pub detail: String,
    pub count: usize,
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL DEFAULT '',
  role TEXT NOT NULL DEFAULT 'unknown',
  discovered_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS groups (
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  owner_user_id INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 0,
  ai_enabled INTEGER NOT NULL DEFAULT 0,
  moderation_enabled INTEGER NOT NULL DEFAULT 0,
  machine_rules_enabled INTEGER NOT NULL DEFAULT 0,
  ai_rules_enabled INTEGER NOT NULL DEFAULT 0,
  manual_takeover INTEGER NOT NULL DEFAULT 0,
  welcome_message TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, group_id),
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS members (
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  nim_id TEXT NOT NULL DEFAULT '',
  nickname TEXT NOT NULL DEFAULT '',
  card_name TEXT NOT NULL DEFAULT '',
  original_card_name TEXT NOT NULL DEFAULT '',
  managed_card_name TEXT NOT NULL DEFAULT '',
  card_suffix TEXT NOT NULL DEFAULT '',
  role TEXT NOT NULL DEFAULT 'member',
  account_state TEXT NOT NULL DEFAULT '',
  blacklisted INTEGER NOT NULL DEFAULT 0,
  present INTEGER NOT NULL DEFAULT 1,
  join_source TEXT NOT NULL DEFAULT 'baseline',
  prompt_read INTEGER NOT NULL DEFAULT 1,
  locked_card_name TEXT NOT NULL DEFAULT '',
  violation_count INTEGER NOT NULL DEFAULT 0,
  discovered_at TEXT NOT NULL DEFAULT '',
  joined_at TEXT,
  last_seen_at TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, group_id, user_id),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS member_identity_aliases (
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  nim_id TEXT NOT NULL,
  user_id INTEGER NOT NULL,
  provisional_user_id INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, group_id, nim_id),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  server_message_id TEXT NOT NULL,
  sequence INTEGER NOT NULL DEFAULT 0,
  user_id INTEGER NOT NULL DEFAULT 0,
  sender_name TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL DEFAULT 'other',
  text TEXT NOT NULL DEFAULT '',
  sent_at TEXT NOT NULL,
  received_at TEXT NOT NULL,
  processed_at TEXT,
  acknowledged_at TEXT,
  processing_state TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  mentions_json TEXT NOT NULL DEFAULT '[]',
  source_kind TEXT,
  flow TEXT,
  UNIQUE(account_id, group_id, server_message_id),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS gateway_inbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  bridge_session TEXT NOT NULL DEFAULT '',
  bridge_sequence INTEGER NOT NULL DEFAULT 0,
  event_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  received_at TEXT NOT NULL,
  claimed_at TEXT,
  processed_at TEXT,
  UNIQUE(account_id, event_id),
  UNIQUE(account_id, bridge_session, bridge_sequence)
);
CREATE TABLE IF NOT EXISTS effect_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL DEFAULT 0,
  effect_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  dedupe_key TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT 'queued',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  receipt_json TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  claimed_at TEXT,
  completed_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS effect_outbox_dedupe_idx
  ON effect_outbox(account_id, dedupe_key) WHERE dedupe_key <> '';
CREATE TABLE IF NOT EXISTS rules (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  rule_type TEXT NOT NULL DEFAULT 'machine',
  scope TEXT NOT NULL DEFAULT 'global',
  priority_level TEXT NOT NULL DEFAULT 'medium',
  group_id INTEGER NOT NULL DEFAULT 0,
  name TEXT NOT NULL,
  matcher TEXT NOT NULL,
  pattern TEXT NOT NULL DEFAULT '',
  threshold INTEGER NOT NULL DEFAULT 0,
  count INTEGER NOT NULL DEFAULT 0,
  window_seconds INTEGER NOT NULL DEFAULT 0,
  cooldown_seconds INTEGER NOT NULL DEFAULT 0,
  priority INTEGER NOT NULL DEFAULT 0,
  mode TEXT NOT NULL DEFAULT 'observe',
  enabled INTEGER NOT NULL DEFAULT 0,
  semantic_threshold REAL NOT NULL DEFAULT 0,
  exempt_roles_json TEXT NOT NULL DEFAULT '[]',
  exempt_user_ids_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rule_groups (
  rule_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  PRIMARY KEY(rule_id, group_id),
  FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE,
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS rule_whitelist_members (
  rule_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  PRIMARY KEY(rule_id, user_id),
  FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS rule_evaluations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL DEFAULT 0,
  message_id INTEGER NOT NULL DEFAULT 0,
  rule_id INTEGER NOT NULL,
  rule_type TEXT NOT NULL,
  matched INTEGER NOT NULL DEFAULT 0,
  confidence REAL,
  mode TEXT NOT NULL,
  decision TEXT NOT NULL,
  reason TEXT NOT NULL DEFAULT '',
  elapsed_ms INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  UNIQUE(account_id, message_id, rule_id),
  FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS rule_actions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  rule_id INTEGER NOT NULL,
  kind TEXT NOT NULL,
  duration_seconds INTEGER NOT NULL DEFAULT 0,
  message TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS rule_runtime_state (
  rule_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  window_started_at TEXT,
  window_count INTEGER NOT NULL DEFAULT 0,
  last_executed_at TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(rule_id, account_id, group_id, user_id),
  FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS knowledge_bases (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 1,
  built_in INTEGER NOT NULL DEFAULT 0,
  read_only INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(account_id, name),
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS knowledge_documents (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  base_id INTEGER NOT NULL,
  title TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'text',
  content TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT '',
  content_hash TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(base_id, content_hash),
  FOREIGN KEY(base_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS knowledge_chunks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id INTEGER NOT NULL,
  chunk_index INTEGER NOT NULL,
  content TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  token_count INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(document_id, chunk_index),
  FOREIGN KEY(document_id) REFERENCES knowledge_documents(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS knowledge_base_groups (
  base_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY(base_id, account_id, group_id),
  FOREIGN KEY(base_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE,
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'pending',
  assignee_id INTEGER NOT NULL DEFAULT 0,
  created_by INTEGER NOT NULL DEFAULT 0,
  due_at TEXT,
  reminder_at TEXT,
  reminder_sent_at TEXT,
  reminder_state TEXT NOT NULL DEFAULT 'pending',
  reminder_attempts INTEGER NOT NULL DEFAULT 0,
  reminder_next_attempt_at TEXT,
  reminder_claimed_at TEXT,
  reminder_last_error TEXT NOT NULL DEFAULT '',
  source_key TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS activities (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 0,
  ai_optimize INTEGER NOT NULL DEFAULT 0,
  ai_instructions TEXT NOT NULL DEFAULT '',
  timezone TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  weekdays_json TEXT NOT NULL DEFAULT '[1,2,3,4,5,6,7]',
  next_run_at TEXT,
  source_key TEXT NOT NULL DEFAULT '',
  deleted_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS activities_source_key_idx ON activities(account_id,source_key) WHERE source_key<>'';
CREATE INDEX IF NOT EXISTS activities_due_idx ON activities(account_id,enabled,next_run_at) WHERE deleted_at IS NULL;
CREATE TABLE IF NOT EXISTS activity_groups (
  activity_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  PRIMARY KEY(activity_id,account_id,group_id),
  FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE,
  FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS activity_times (
  activity_id INTEGER NOT NULL,
  local_time TEXT NOT NULL,
  PRIMARY KEY(activity_id,local_time),
  FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS activity_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  activity_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  scheduled_for TEXT NOT NULL,
  run_key TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL DEFAULT 'pending',
  text TEXT NOT NULL DEFAULT '',
  content_source TEXT NOT NULL DEFAULT 'fixed',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE,
  FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS activity_runs_history_idx ON activity_runs(account_id,activity_id,scheduled_for DESC,id DESC);
CREATE TABLE IF NOT EXISTS group_ai_permissions (
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  reply INTEGER NOT NULL DEFAULT 0,
  tasks INTEGER NOT NULL DEFAULT 1,
  recall INTEGER NOT NULL DEFAULT 0,
  mute INTEGER NOT NULL DEFAULT 0,
  remove_member INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, group_id),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS daily_summaries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  local_date TEXT NOT NULL,
  content TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'ai',
  created_at TEXT NOT NULL,
  UNIQUE(account_id, group_id, local_date),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS ai_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  run_key TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'processing',
  attempts INTEGER NOT NULL DEFAULT 1,
  next_retry_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE(account_id, run_key),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS summary_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  run_key TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'processing',
  attempts INTEGER NOT NULL DEFAULT 1,
  next_retry_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE(account_id, group_id, run_key),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS card_rename_jobs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  nim_id TEXT NOT NULL DEFAULT '',
  original_name TEXT NOT NULL DEFAULT '',
  desired_name TEXT NOT NULL,
  suffix TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT 'queued',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  welcome_pending INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(account_id, group_id, user_id, desired_name),
  FOREIGN KEY(account_id, group_id, user_id) REFERENCES members(account_id, group_id, user_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS group_schedules (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  open_time TEXT NOT NULL,
  close_time TEXT NOT NULL,
  timezone TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(account_id, name),
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS group_schedule_groups (
  schedule_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  PRIMARY KEY(schedule_id, account_id, group_id),
  FOREIGN KEY(schedule_id) REFERENCES group_schedules(id) ON DELETE CASCADE,
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS schedule_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  schedule_id INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  local_date TEXT NOT NULL,
  action TEXT NOT NULL,
  run_key TEXT NOT NULL UNIQUE,
  success INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT '',
  attempts INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY(schedule_id) REFERENCES group_schedules(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS actions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL DEFAULT 0,
  message_id INTEGER,
  rule_id INTEGER,
  kind TEXT NOT NULL,
  mode TEXT NOT NULL DEFAULT 'observe',
  duration_seconds INTEGER NOT NULL DEFAULT 0,
  reason TEXT NOT NULL DEFAULT '',
  success INTEGER NOT NULL DEFAULT 0,
  error TEXT NOT NULL DEFAULT '',
  receipt_json TEXT NOT NULL DEFAULT '',
  dedupe_key TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL DEFAULT 0,
  user_id INTEGER NOT NULL DEFAULT 0,
  actor TEXT NOT NULL,
  event TEXT NOT NULL,
  level TEXT NOT NULL DEFAULT 'info',
  details TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL DEFAULT '',
  sensitive INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS expected_member_card_updates (
  account_id TEXT NOT NULL,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  card_name TEXT NOT NULL,
  source_key TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(account_id, group_id, user_id, card_name)
);
CREATE TABLE IF NOT EXISTS ai_provider_endpoints (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  name TEXT NOT NULL,
  base_url TEXT NOT NULL DEFAULT '',
  webhook_url TEXT NOT NULL DEFAULT '',
  api_backend TEXT NOT NULL DEFAULT 'chat_completions',
  model TEXT NOT NULL DEFAULT 'deepseek-v4-pro',
  reasoning_effort TEXT NOT NULL DEFAULT 'low',
  secret_ref TEXT NOT NULL DEFAULT '',
  priority INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  health_status TEXT NOT NULL DEFAULT 'unchecked',
  failure_count INTEGER NOT NULL DEFAULT 0,
  cooldown_until TEXT,
  last_error TEXT NOT NULL DEFAULT '',
  last_checked_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS ai_provider_endpoints_account_priority_idx
  ON ai_provider_endpoints(account_id, enabled DESC, priority, id);
CREATE TABLE IF NOT EXISTS business_apps (
  account_id TEXT NOT NULL,
  app_id TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  version TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'unchecked',
  status_detail TEXT NOT NULL DEFAULT '',
  last_checked_at TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, app_id),
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS business_app_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,
  app_id TEXT NOT NULL,
  group_id INTEGER NOT NULL DEFAULT 0,
  message_id INTEGER NOT NULL DEFAULT 0,
  run_key TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'processing',
  freshness TEXT NOT NULL DEFAULT 'missing',
  ai_used INTEGER NOT NULL DEFAULT 0,
  reply TEXT NOT NULL DEFAULT '',
  error TEXT NOT NULL DEFAULT '',
  elapsed_ms INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE(account_id, app_id, run_key),
  FOREIGN KEY(account_id, app_id) REFERENCES business_apps(account_id, app_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS gateway_capability_verifications (
  fingerprint TEXT NOT NULL,
  capability TEXT NOT NULL,
  source TEXT NOT NULL,
  status TEXT NOT NULL,
  automatic_allowed INTEGER NOT NULL DEFAULT 0,
  evidence_hash TEXT NOT NULL DEFAULT '',
  last_error TEXT NOT NULL DEFAULT '',
  verified_at TEXT NOT NULL,
  PRIMARY KEY(fingerprint, capability)
);
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDepthRow {
    pub state: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicyRow {
    pub table_name: String,
    pub max_days: i64,
    pub max_rows: i64,
    pub batch_size: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatus {
    pub path: String,
    pub schema_version: i64,
    pub integrity: String,
    pub accounts: i64,
    pub groups: i64,
    pub messages: i64,
    /// Per-state row counts for `effect_outbox` (queued/retry/processing/failed/succeeded/unknown).
    pub queue_depth: Vec<QueueDepthRow>,
    /// Current rows from the `retention_policies` housekeeping table.
    pub retention_policies: Vec<RetentionPolicyRow>,
    /// Result of `PRAGMA integrity_check(16)` — "ok" when the database is healthy.
    pub index_integrity: String,
}

pub struct Database {
    pub(crate) path: PathBuf,
    pub(crate) connection: Arc<Mutex<Connection>>,
}

type DatabaseJob = Box<dyn FnOnce(&Database) + Send + 'static>;

enum DatabaseCommand {
    Run(DatabaseJob),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

struct DatabaseExecutorInner {
    sender: tokio::sync::mpsc::Sender<DatabaseCommand>,
    enqueue_gate: tokio::sync::Mutex<()>,
    stopped: AtomicBool,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Serializes async database work onto one dedicated OS thread.
///
/// `Database` keeps its synchronous API for startup, migrations and existing
/// commands. Long-running Tokio/Tauri tasks should use this executor so a
/// rusqlite call never blocks an async worker while waiting for the connection.
#[derive(Clone)]
pub struct DatabaseExecutor {
    inner: Arc<DatabaseExecutorInner>,
}

impl DatabaseExecutor {
    pub fn start(database: Database) -> AppResult<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<DatabaseCommand>(256);
        let worker = std::thread::Builder::new()
            .name("dh-sqlite".into())
            .spawn(move || {
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        DatabaseCommand::Run(job) => job(&database),
                        DatabaseCommand::Shutdown(acknowledge) => {
                            let _ = acknowledge.send(());
                            break;
                        }
                    }
                }
            })
            .map_err(|error| {
                AppError::new(
                    "database_executor_start",
                    format!("启动 SQLite 执行器失败：{error}"),
                )
            })?;
        Ok(Self {
            inner: Arc::new(DatabaseExecutorInner {
                sender,
                enqueue_gate: tokio::sync::Mutex::new(()),
                stopped: AtomicBool::new(false),
                worker: Mutex::new(Some(worker)),
            }),
        })
    }

    pub async fn execute<T, F>(&self, operation: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Database) -> AppResult<T> + Send + 'static,
    {
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        {
            let _gate = self.inner.enqueue_gate.lock().await;
            if self.inner.stopped.load(Ordering::Acquire) {
                return Err(AppError::new(
                    "database_executor_stopped",
                    "SQLite 执行器已停止，请重启 DH BOT",
                ));
            }
            self.inner
                .sender
                .send(DatabaseCommand::Run(Box::new(move |database| {
                    let mut result = operation(database);
                    if let Err(error) = &mut result {
                        if is_disk_io_error(error) {
                            match database.protect_snapshot_after_io_error() {
                                Ok(path) => {
                                    error.detail_ref = Some(path.display().to_string());
                                    error.message = format!(
                                        "{}；原始数据库未修改，已创建保护备份：{}",
                                        error.message,
                                        path.display()
                                    );
                                }
                                Err(snapshot_error) => {
                                    error.message = format!(
                                        "{}；保护备份失败：{}",
                                        error.message, snapshot_error.message
                                    );
                                }
                            }
                        }
                    }
                    let _ = result_sender.send(result);
                })))
                .await
                .map_err(|_| {
                    AppError::new(
                        "database_executor_stopped",
                        "SQLite 执行器已停止，请重启 DH BOT",
                    )
                })?;
        }
        result_receiver.await.map_err(|_| {
            AppError::new(
                "database_executor_cancelled",
                "SQLite 执行任务未返回结果，请重试",
            )
        })?
    }

    /// Stops accepting new work, drains every command already queued before
    /// this call, then joins the dedicated SQLite thread.
    pub async fn shutdown(&self) -> AppResult<()> {
        let (acknowledge_sender, acknowledge_receiver) = tokio::sync::oneshot::channel();
        {
            let _gate = self.inner.enqueue_gate.lock().await;
            if self.inner.stopped.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            self.inner
                .sender
                .send(DatabaseCommand::Shutdown(acknowledge_sender))
                .await
                .map_err(|_| {
                    AppError::new(
                        "database_executor_stopped",
                        "SQLite 执行器已停止，请重启 DH BOT",
                    )
                })?;
        }
        acknowledge_receiver.await.map_err(|_| {
            AppError::new("database_executor_shutdown", "SQLite 执行器关闭回执丢失")
        })?;
        let worker = self
            .inner
            .worker
            .lock()
            .map_err(|_| AppError::new("database_executor_shutdown", "SQLite 线程锁已损坏"))?
            .take();
        if let Some(worker) = worker {
            tokio::task::spawn_blocking(move || worker.join())
                .await
                .map_err(|error| {
                    AppError::new(
                        "database_executor_shutdown",
                        format!("SQLite 执行线程等待失败：{error}"),
                    )
                })?
                .map_err(|_| {
                    AppError::new("database_executor_shutdown", "SQLite 执行线程异常退出")
                })?;
        }
        Ok(())
    }

    pub async fn status(&self) -> AppResult<DatabaseStatus> {
        self.execute(Database::status).await
    }

    pub async fn runtime_backlog(&self) -> AppResult<Vec<RuntimeBacklog>> {
        self.execute(Database::runtime_backlog).await
    }

    pub async fn ingest_gateway_batch(
        &self,
        events: Vec<GatewayInboxEvent>,
        messages: Vec<Message>,
    ) -> AppResult<(BatchIngestResult, Vec<PersistedMessage>)> {
        self.execute(move |database| database.ingest_gateway_batch(&events, &messages))
            .await
    }

    pub async fn claim_gateway_inbox(
        &self,
        account_id: Option<String>,
        limit: usize,
    ) -> AppResult<Vec<GatewayInboxItem>> {
        self.execute(move |database| database.claim_gateway_inbox(account_id.as_deref(), limit))
            .await
    }

    pub async fn finish_gateway_inbox(
        &self,
        id: i64,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_gateway_inbox(id, success, &error))
            .await
    }

    pub async fn ignore_gateway_inbox_event(
        &self,
        account_id: String,
        event_id: String,
        reason: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.ignore_gateway_inbox_event(&account_id, &event_id, &reason)
        })
        .await
    }

    pub async fn mark_gateway_inbox_processed(
        &self,
        account_id: String,
        event_ids: Vec<String>,
    ) -> AppResult<usize> {
        self.execute(move |database| database.mark_gateway_inbox_processed(&account_id, &event_ids))
            .await
    }

    pub async fn processed_gateway_event_ids(
        &self,
        account_id: String,
        event_ids: Vec<String>,
    ) -> AppResult<Vec<String>> {
        self.execute(move |database| database.processed_gateway_event_ids(&account_id, &event_ids))
            .await
    }

    pub async fn commit_gateway_ack(
        &self,
        account_id: String,
        event_ids: Vec<String>,
        message_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.commit_gateway_ack(&account_id, &event_ids, &message_ids)
        })
        .await
    }

    pub async fn enqueue_effect(&self, request: EffectOutboxRequest) -> AppResult<EnqueuedEffect> {
        self.execute(move |database| database.enqueue_effect(&request))
            .await
    }

    pub async fn enqueue_activity_effect(
        &self,
        run_id: i64,
        text: String,
        source: String,
        request: EffectOutboxRequest,
    ) -> AppResult<EnqueuedEffect> {
        self.execute(move |database| {
            database.enqueue_activity_effect(run_id, &text, &source, &request)
        })
        .await
    }

    pub async fn claim_effect_outbox(
        &self,
        account_id: Option<String>,
        limit: usize,
    ) -> AppResult<Vec<EffectOutboxItem>> {
        self.execute(move |database| database.claim_effect_outbox(account_id.as_deref(), limit))
            .await
    }

    pub async fn finish_effect_outbox(
        &self,
        id: i64,
        success: bool,
        error: String,
        receipt_json: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.finish_effect_outbox(id, success, &error, &receipt_json)
        })
        .await
    }

    pub async fn finish_effect_outbox_unknown(
        &self,
        id: i64,
        error: String,
        receipt_json: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.finish_effect_outbox_unknown(id, &error, &receipt_json)
        })
        .await
    }

    pub async fn mark_processing_effects_unknown(&self, reason: String) -> AppResult<usize> {
        self.execute(move |database| database.mark_processing_effects_unknown(&reason))
            .await
    }

    pub async fn mark_message_acknowledged(&self, id: i64) -> AppResult<()> {
        self.execute(move |database| database.mark_message_acknowledged(id))
            .await
    }

    pub async fn claim_message(&self, id: i64) -> AppResult<bool> {
        self.execute(move |database| database.claim_message(id))
            .await
    }

    pub async fn finish_message_processing(
        &self,
        id: i64,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_message_processing(id, success, &error))
            .await
    }

    pub async fn list_pending_messages(
        &self,
        account_id: String,
        limit: usize,
    ) -> AppResult<Vec<Message>> {
        self.execute(move |database| database.list_pending_messages(&account_id, limit))
            .await
    }

    pub async fn get_setting(&self, key: String) -> AppResult<Option<String>> {
        self.execute(move |database| database.get_setting(&key))
            .await
    }

    pub async fn ensure_ai_provider_endpoints(&self, account_id: String) -> AppResult<()> {
        self.execute(move |database| database.ensure_ai_provider_endpoints(&account_id))
            .await
    }

    pub async fn list_ai_provider_endpoints(
        &self,
        account_id: String,
    ) -> AppResult<Vec<AiProviderEndpoint>> {
        self.execute(move |database| database.list_ai_provider_endpoints(&account_id))
            .await
    }

    pub async fn save_ai_provider_endpoint(&self, endpoint: AiProviderEndpoint) -> AppResult<i64> {
        self.execute(move |database| database.save_ai_provider_endpoint(&endpoint))
            .await
    }

    pub async fn delete_ai_provider_endpoint(
        &self,
        account_id: String,
        endpoint_id: i64,
    ) -> AppResult<()> {
        self.execute(move |database| database.delete_ai_provider_endpoint(&account_id, endpoint_id))
            .await
    }

    pub async fn update_ai_provider_health(
        &self,
        account_id: String,
        endpoint_id: i64,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.update_ai_provider_health(&account_id, endpoint_id, success, &error)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_gateway_capability_verification(
        &self,
        fingerprint: String,
        capability: String,
        source: String,
        status: String,
        automatic_allowed: bool,
        evidence_hash: String,
        last_error: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.save_gateway_capability_verification(
                &fingerprint,
                &capability,
                &source,
                &status,
                automatic_allowed,
                &evidence_hash,
                &last_error,
            )
        })
        .await
    }

    pub async fn is_gateway_capability_verified(
        &self,
        fingerprint: String,
        capability: String,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.is_gateway_capability_verified(&fingerprint, &capability)
        })
        .await
    }

    pub async fn upsert_account(&self, account: Account) -> AppResult<()> {
        self.execute(move |database| database.upsert_account(&account))
            .await
    }

    pub async fn ensure_account_defaults(&self, account_id: String) -> AppResult<()> {
        self.execute(move |database| database.ensure_account_defaults(&account_id))
            .await
    }

    pub async fn ensure_business_apps(&self, account_id: String) -> AppResult<()> {
        self.execute(move |database| database.ensure_business_apps(&account_id))
            .await
    }

    pub async fn list_business_apps(
        &self,
        account_id: String,
    ) -> AppResult<Vec<BusinessAppRecord>> {
        self.execute(move |database| database.list_business_apps(&account_id))
            .await
    }

    pub async fn set_business_app_enabled(
        &self,
        account_id: String,
        app_id: String,
        enabled: bool,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.set_business_app_enabled(&account_id, &app_id, enabled)
        })
        .await
    }

    pub async fn business_app_enabled(
        &self,
        account_id: String,
        app_id: String,
    ) -> AppResult<bool> {
        self.execute(move |database| database.business_app_enabled(&account_id, &app_id))
            .await
    }

    pub async fn update_business_app_health(
        &self,
        account_id: String,
        app_id: String,
        status: String,
        detail: String,
        checked_at: DateTime<Utc>,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.update_business_app_health(&account_id, &app_id, &status, &detail, checked_at)
        })
        .await
    }

    pub async fn claim_business_app_run(&self, run: BusinessAppRun) -> AppResult<bool> {
        self.execute(move |database| database.claim_business_app_run(&run))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_business_app_run(
        &self,
        account_id: String,
        app_id: String,
        run_key: String,
        status: String,
        freshness: String,
        ai_used: bool,
        reply: String,
        error: String,
        elapsed_ms: i64,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.finish_business_app_run(
                &account_id,
                &app_id,
                &run_key,
                &status,
                &freshness,
                ai_used,
                &reply,
                &error,
                elapsed_ms,
            )
        })
        .await
    }

    pub async fn list_business_app_runs(
        &self,
        account_id: String,
        app_id: String,
        limit: usize,
    ) -> AppResult<Vec<BusinessAppRun>> {
        self.execute(move |database| database.list_business_app_runs(&account_id, &app_id, limit))
            .await
    }

    pub async fn upsert_group(&self, group: Group) -> AppResult<()> {
        self.execute(move |database| database.upsert_group(&group))
            .await
    }

    pub async fn list_groups(&self, account_id: Option<String>) -> AppResult<Vec<Group>> {
        self.execute(move |database| database.list_groups(account_id.as_deref()))
            .await
    }

    pub async fn upsert_member(&self, member: Member) -> AppResult<i64> {
        self.execute(move |database| database.upsert_member(&member))
            .await
    }

    pub async fn list_members(&self, account_id: String, group_id: i64) -> AppResult<Vec<Member>> {
        self.execute(move |database| database.list_members(&account_id, group_id))
            .await
    }

    pub async fn resolve_member_user_id(
        &self,
        account_id: String,
        group_id: i64,
        nim_id: String,
    ) -> AppResult<Option<i64>> {
        self.execute(move |database| {
            database.resolve_member_user_id(&account_id, group_id, &nim_id)
        })
        .await
    }

    pub async fn set_member_blacklisted(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
        blacklisted: bool,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.set_member_blacklisted(&account_id, group_id, user_id, blacklisted)
        })
        .await
    }

    pub async fn increment_member_violation(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<i64> {
        self.execute(move |database| {
            database.increment_member_violation(&account_id, group_id, user_id)
        })
        .await
    }

    pub async fn expect_member_card_update(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
        card_name: String,
        source_key: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.expect_member_card_update(
                &account_id,
                group_id,
                user_id,
                &card_name,
                &source_key,
            )
        })
        .await
    }

    pub async fn consume_expected_member_card_update(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
        card_name: String,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.consume_expected_member_card_update(&account_id, group_id, user_id, &card_name)
        })
        .await
    }

    pub async fn mark_member_not_present(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.mark_member_not_present(&account_id, group_id, user_id)
        })
        .await
    }

    pub async fn mark_members_not_present(
        &self,
        account_id: String,
        group_id: i64,
        present_user_ids: Vec<i64>,
    ) -> AppResult<Vec<Member>> {
        self.execute(move |database| {
            database.mark_members_not_present(&account_id, group_id, &present_user_ids)
        })
        .await
    }

    pub async fn mark_members_not_present_before(
        &self,
        account_id: String,
        group_id: i64,
        present_user_ids: Vec<i64>,
        before: DateTime<Utc>,
    ) -> AppResult<Vec<Member>> {
        self.execute(move |database| {
            database.mark_members_not_present_before(
                &account_id,
                group_id,
                &present_user_ids,
                &before,
            )
        })
        .await
    }

    pub async fn enqueue_card_job(
        &self,
        account_id: String,
        group_id: i64,
        plan: CardPlan,
        welcome_pending: bool,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.enqueue_card_job(&account_id, group_id, &plan, welcome_pending)
        })
        .await
    }

    pub async fn claim_next_card_job(
        &self,
        account_id: String,
    ) -> AppResult<Option<CardRenameJob>> {
        self.execute(move |database| database.claim_next_card_job(&account_id))
            .await
    }

    pub async fn finish_card_job(
        &self,
        job: CardRenameJob,
        success: bool,
        retryable: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_card_job(&job, success, retryable, &error))
            .await
    }

    pub async fn mark_card_welcome_sent(&self, job_id: i64) -> AppResult<()> {
        self.execute(move |database| database.mark_card_welcome_sent(job_id))
            .await
    }

    pub async fn list_card_jobs(
        &self,
        account_id: String,
        group_id: i64,
        limit: usize,
    ) -> AppResult<Vec<CardRenameJob>> {
        self.execute(move |database| database.list_card_jobs(&account_id, group_id, limit))
            .await
    }

    pub async fn retry_failed_card_jobs(
        &self,
        account_id: String,
        group_id: i64,
    ) -> AppResult<usize> {
        self.execute(move |database| database.retry_failed_card_jobs(&account_id, group_id))
            .await
    }

    pub async fn insert_message(&self, message: Message) -> AppResult<PersistedMessage> {
        self.execute(move |database| database.insert_message(&message))
            .await
    }

    pub async fn list_messages(
        &self,
        account_id: String,
        group_id: Option<i64>,
        limit: usize,
    ) -> AppResult<Vec<Message>> {
        self.execute(move |database| database.list_messages(&account_id, group_id, limit))
            .await
    }

    pub async fn recent_messages(
        &self,
        account_id: String,
        group_id: i64,
        limit: usize,
    ) -> AppResult<Vec<Message>> {
        self.execute(move |database| database.recent_messages(&account_id, group_id, limit))
            .await
    }

    pub async fn list_rules(
        &self,
        account_id: String,
        group_id: Option<i64>,
    ) -> AppResult<Vec<ModerationRule>> {
        self.execute(move |database| database.list_rules(&account_id, group_id))
            .await
    }

    pub async fn message_processing_context(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<Option<MessageProcessingContext>> {
        self.execute(move |database| {
            let Some(group) = database.get_group(&account_id, group_id)? else {
                return Ok(None);
            };
            let member = database.get_member(&account_id, group_id, user_id)?;
            let recent_messages = database.recent_messages(&account_id, group_id, 20)?;
            let rules = if group.machine_rules_enabled || group.ai_rules_enabled {
                database.list_rules(&account_id, Some(group_id))?
            } else {
                Vec::new()
            };
            Ok(Some(MessageProcessingContext {
                group,
                member,
                recent_messages,
                rules,
            }))
        })
        .await
    }

    pub async fn rule_cooldown_allows(
        &self,
        rule_id: i64,
        account_id: String,
        group_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
        cooldown_seconds: i64,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.rule_cooldown_allows(
                rule_id,
                &account_id,
                group_id,
                user_id,
                now,
                cooldown_seconds,
            )
        })
        .await
    }

    pub async fn mark_rule_executed(
        &self,
        rule_id: i64,
        account_id: String,
        group_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.mark_rule_executed(rule_id, &account_id, group_id, user_id, now)
        })
        .await
    }

    pub async fn record_action(&self, action: ActionRecord) -> AppResult<i64> {
        self.execute(move |database| database.record_action(&action))
            .await
    }

    pub async fn action_succeeded(
        &self,
        account_id: String,
        dedupe_key: String,
    ) -> AppResult<bool> {
        self.execute(move |database| database.action_succeeded(&account_id, &dedupe_key))
            .await
    }

    pub async fn record_audit(&self, event: AuditEvent) -> AppResult<i64> {
        self.execute(move |database| database.record_audit(&event))
            .await
    }

    pub async fn group_ai_permissions(
        &self,
        account_id: String,
        group_id: i64,
    ) -> AppResult<Option<GroupAiPermissions>> {
        self.execute(move |database| database.group_ai_permissions(&account_id, group_id))
            .await
    }

    pub async fn list_knowledge_for_group(
        &self,
        account_id: String,
        group_id: i64,
    ) -> AppResult<Vec<KnowledgeDocument>> {
        self.execute(move |database| database.list_knowledge_for_group(&account_id, group_id))
            .await
    }

    pub async fn save_task_once(&self, task: TaskItem, source_key: String) -> AppResult<i64> {
        self.execute(move |database| database.save_task_once(&task, &source_key))
            .await
    }


    pub async fn finish_task_reminder(
        &self,
        task_id: i64,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_task_reminder(task_id, success, &error))
            .await
    }

    pub async fn mark_task_reminder_unknown(&self, task_id: i64, error: String) -> AppResult<()> {
        self.execute(move |database| database.mark_task_reminder_unknown(task_id, &error))
            .await
    }

    pub async fn save_daily_summary(&self, summary: DailySummary) -> AppResult<i64> {
        self.execute(move |database| database.save_daily_summary(&summary))
            .await
    }

    pub async fn list_daily_summaries(
        &self,
        account_id: String,
        limit: usize,
    ) -> AppResult<Vec<DailySummary>> {
        self.execute(move |database| database.list_daily_summaries(&account_id, limit))
            .await
    }

    pub async fn claim_ai_run(
        &self,
        account_id: String,
        group_id: i64,
        run_key: String,
    ) -> AppResult<bool> {
        self.execute(move |database| database.claim_ai_run(&account_id, group_id, &run_key))
            .await
    }

    pub async fn finish_ai_run(
        &self,
        account_id: String,
        run_key: String,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_ai_run(&account_id, &run_key, success, &error))
            .await
    }

    pub async fn fail_ai_run_terminal(
        &self,
        account_id: String,
        run_key: String,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.fail_ai_run_terminal(&account_id, &run_key, &error))
            .await
    }

    pub async fn claim_summary_run(
        &self,
        account_id: String,
        group_id: i64,
        run_key: String,
    ) -> AppResult<bool> {
        self.execute(move |database| database.claim_summary_run(&account_id, group_id, &run_key))
            .await
    }

    pub async fn finish_summary_run(
        &self,
        account_id: String,
        group_id: i64,
        run_key: String,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.finish_summary_run(&account_id, group_id, &run_key, success, &error)
        })
        .await
    }

    pub async fn list_schedules(&self, account_id: String) -> AppResult<Vec<GroupSchedule>> {
        self.execute(move |database| database.list_schedules(&account_id))
            .await
    }

    pub async fn claim_schedule_run(
        &self,
        schedule_id: i64,
        account_id: String,
        group_id: i64,
        action: String,
        run_key: String,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.claim_schedule_run(schedule_id, &account_id, group_id, &action, &run_key)
        })
        .await
    }

    pub async fn finish_schedule_run(
        &self,
        run_key: String,
        success: bool,
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_schedule_run(&run_key, success, &error))
            .await
    }

    pub async fn mark_schedule_run_unknown(&self, run_key: String, error: String) -> AppResult<()> {
        self.execute(move |database| database.mark_schedule_run_unknown(&run_key, &error))
            .await
    }
}

fn is_disk_io_error(error: &AppError) -> bool {
    let text = error.message.to_ascii_lowercase();
    text.contains("disk i/o error")
        || text.contains("database disk image is malformed")
        || text.contains("database or disk is full")
}

fn protect_database_files(path: &Path, root: &Path) -> AppResult<PathBuf> {
    let directory = root
        .join("backups")
        .join(format!("dh-io-{}", Utc::now().format("%Y%m%d-%H%M%S-%3f")));
    fs::create_dir_all(&directory)
        .map_err(|error| AppError::new("database_backup", error.to_string()))?;
    let mut copied = 0_u8;
    for suffix in ["", "-wal", "-shm"] {
        let source = PathBuf::from(format!("{}{}", path.display(), suffix));
        if !source.exists() {
            continue;
        }
        let file_name = source
            .file_name()
            .ok_or_else(|| AppError::new("database_backup", "SQLite 文件名不可用"))?;
        fs::copy(&source, directory.join(file_name))
            .map_err(|error| AppError::new("database_backup", error.to_string()))?;
        copied = copied.saturating_add(1);
    }
    if copied == 0 {
        return Err(AppError::new(
            "database_backup",
            "未找到可复制的 SQLite 文件",
        ));
    }
    Ok(directory)
}

fn snapshot_before_migration(
    paths: &AppPaths,
    connection: &Connection,
    version: i64,
) -> AppResult<PathBuf> {
    let directory = paths.v3.join("backups");
    std::fs::create_dir_all(&directory)
        .map_err(|error| AppError::new("database_backup", error.to_string()))?;
    let path = directory.join(format!(
        "dh-v{version}-{}.db",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let quoted = path.to_string_lossy().replace('\'', "''");
    connection
        .execute_batch(&format!("VACUUM INTO '{quoted}';"))
        .map_err(|error| {
            AppError::new("database_backup", format!("迁移前数据库快照失败：{error}"))
        })?;
    let mut snapshots = std::fs::read_dir(&directory)
        .map_err(|error| AppError::new("database_backup", error.to_string()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("dh-v"))
        .collect::<Vec<_>>();
    snapshots.sort_by_key(|entry| entry.file_name());
    let remove_count = snapshots.len().saturating_sub(3);
    for entry in snapshots.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(entry.path());
    }
    Ok(path)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let found = columns.filter_map(Result::ok).any(|value| value == column);
    Ok(found)
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    if !table_has_column(connection, table, column)? {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn quick_check_rows(connection: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut statement = connection.prepare("PRAGMA quick_check")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn quick_check_connection(connection: &Connection) -> AppResult<()> {
    let rows = quick_check_rows(connection).map_err(|error| {
        AppError::new(
            "database_corrupt",
            format!("SQLite 完整性检查失败：{error}"),
        )
    })?;
    if rows.len() == 1 && rows[0] == "ok" {
        return Ok(());
    }
    Err(AppError::new(
        "database_corrupt",
        format!("SQLite 完整性检查失败：{}", rows.join("；")),
    ))
}

fn migrate_schema(connection: &mut Connection, old_version: i64) -> AppResult<()> {
    if old_version > 14 {
        return Err(AppError::new(
            "database_version",
            format!("数据库版本 {old_version} 高于当前程序支持的 v14"),
        ));
    }
    let transaction = connection
        .transaction()
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    // Older development databases were not consistent about their user_version.
    // Inspecting table_info makes this migration safe to run repeatedly on v0/v1/v2.
    for (table, column, definition) in [
        ("members", "join_source", "TEXT NOT NULL DEFAULT 'baseline'"),
        ("members", "prompt_read", "INTEGER NOT NULL DEFAULT 1"),
        ("members", "locked_card_name", "TEXT NOT NULL DEFAULT ''"),
        ("members", "violation_count", "INTEGER NOT NULL DEFAULT 0"),
        ("members", "discovered_at", "TEXT NOT NULL DEFAULT ''"),
        ("members", "joined_at", "TEXT"),
        ("members", "last_seen_at", "TEXT NOT NULL DEFAULT ''"),
        (
            "messages",
            "processing_state",
            "TEXT NOT NULL DEFAULT 'pending'",
        ),
        ("messages", "attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("messages", "next_attempt_at", "TEXT"),
        ("messages", "last_error", "TEXT NOT NULL DEFAULT ''"),
        ("messages", "mentions_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("messages", "source_kind", "TEXT"),
        ("messages", "flow", "TEXT"),
        (
            "gateway_inbox",
            "bridge_session",
            "TEXT NOT NULL DEFAULT ''",
        ),
        (
            "gateway_inbox",
            "bridge_sequence",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("actions", "dedupe_key", "TEXT NOT NULL DEFAULT ''"),
        ("actions", "receipt_json", "TEXT NOT NULL DEFAULT ''"),
        ("effect_outbox", "receipt_json", "TEXT NOT NULL DEFAULT ''"),
        (
            "knowledge_documents",
            "enabled",
            "INTEGER NOT NULL DEFAULT 1",
        ),
        ("tasks", "assignee_id", "INTEGER NOT NULL DEFAULT 0"),
        ("tasks", "created_by", "INTEGER NOT NULL DEFAULT 0"),
        ("tasks", "reminder_at", "TEXT"),
        ("tasks", "reminder_sent_at", "TEXT"),
        ("tasks", "reminder_state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("tasks", "reminder_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("tasks", "reminder_next_attempt_at", "TEXT"),
        ("tasks", "reminder_claimed_at", "TEXT"),
        ("tasks", "reminder_last_error", "TEXT NOT NULL DEFAULT ''"),
        ("tasks", "source_key", "TEXT NOT NULL DEFAULT ''"),
        (
            "groups",
            "machine_rules_enabled",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("groups", "ai_rules_enabled", "INTEGER NOT NULL DEFAULT 0"),
        ("rules", "rule_type", "TEXT NOT NULL DEFAULT 'machine'"),
        ("rules", "scope", "TEXT NOT NULL DEFAULT 'global'"),
        ("rules", "priority_level", "TEXT NOT NULL DEFAULT 'medium'"),
    ] {
        add_column_if_missing(&transaction, table, column, definition).map_err(|error| {
            AppError::new(
                "database_migration",
                format!("补齐 {table}.{column} 失败：{error}"),
            )
        })?;
    }
    transaction
        .execute(
            "UPDATE groups SET machine_rules_enabled=moderation_enabled WHERE machine_rules_enabled=0 AND moderation_enabled=1",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            "UPDATE rules SET rule_type=CASE WHEN matcher='semantic' THEN 'ai' ELSE 'machine' END,scope=CASE WHEN group_id=0 THEN 'global' ELSE 'selected' END,priority_level=CASE WHEN priority<100 THEN 'low' WHEN priority<200 THEN 'medium' ELSE 'high' END",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rule_groups (rule_id INTEGER NOT NULL,account_id TEXT NOT NULL,group_id INTEGER NOT NULL,PRIMARY KEY(rule_id,group_id),FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE,FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS rule_whitelist_members (rule_id INTEGER NOT NULL,user_id INTEGER NOT NULL,PRIMARY KEY(rule_id,user_id),FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS rule_evaluations (id INTEGER PRIMARY KEY AUTOINCREMENT,account_id TEXT NOT NULL,group_id INTEGER NOT NULL,user_id INTEGER NOT NULL DEFAULT 0,message_id INTEGER NOT NULL DEFAULT 0,rule_id INTEGER NOT NULL,rule_type TEXT NOT NULL,matched INTEGER NOT NULL DEFAULT 0,confidence REAL,mode TEXT NOT NULL,decision TEXT NOT NULL,reason TEXT NOT NULL DEFAULT '',elapsed_ms INTEGER NOT NULL DEFAULT 0,created_at TEXT NOT NULL,UNIQUE(account_id,message_id,rule_id),FOREIGN KEY(rule_id) REFERENCES rules(id) ON DELETE CASCADE);
             INSERT OR IGNORE INTO rule_groups(rule_id,account_id,group_id) SELECT id,account_id,group_id FROM rules WHERE group_id>0;
             INSERT OR IGNORE INTO rule_whitelist_members(rule_id,user_id) SELECT rules.id,CAST(json_each.value AS INTEGER) FROM rules,json_each(rules.exempt_user_ids_json) WHERE CAST(json_each.value AS INTEGER)>0;"
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            "UPDATE members SET discovered_at=COALESCE(NULLIF(discovered_at,''),updated_at), last_seen_at=COALESCE(NULLIF(last_seen_at,''),updated_at)",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            "UPDATE messages SET processing_state=CASE WHEN processed_at IS NOT NULL THEN 'processed' ELSE 'pending' END WHERE processing_state IS NULL OR processing_state=''",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            "UPDATE tasks SET reminder_state=CASE WHEN reminder_sent_at IS NOT NULL THEN 'sent' ELSE 'pending' END WHERE reminder_state IS NULL OR reminder_state='' OR reminder_sent_at IS NOT NULL",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO member_identity_aliases(account_id,group_id,nim_id,user_id,provisional_user_id,created_at,updated_at) SELECT account_id,group_id,nim_id,user_id,CASE WHEN user_id<0 THEN user_id ELSE NULL END,COALESCE(NULLIF(discovered_at,''),updated_at),updated_at FROM members WHERE nim_id<>''",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS actions_dedupe_key_idx ON actions(account_id, dedupe_key) WHERE dedupe_key <> ''")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS gateway_inbox_bridge_idx ON gateway_inbox(account_id,bridge_session,bridge_sequence) WHERE bridge_session<>'' AND bridge_sequence>0")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS tasks_source_key_idx ON tasks(account_id,source_key) WHERE source_key<>''")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    let migration_timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS activities (id INTEGER PRIMARY KEY AUTOINCREMENT,account_id TEXT NOT NULL,name TEXT NOT NULL,content TEXT NOT NULL DEFAULT '',enabled INTEGER NOT NULL DEFAULT 0,ai_optimize INTEGER NOT NULL DEFAULT 0,ai_instructions TEXT NOT NULL DEFAULT '',timezone TEXT NOT NULL,start_date TEXT NOT NULL,end_date TEXT NOT NULL,weekdays_json TEXT NOT NULL DEFAULT '[1,2,3,4,5,6,7]',next_run_at TEXT,source_key TEXT NOT NULL DEFAULT '',deleted_at TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE);
             CREATE UNIQUE INDEX IF NOT EXISTS activities_source_key_idx ON activities(account_id,source_key) WHERE source_key<>'';
             CREATE INDEX IF NOT EXISTS activities_due_idx ON activities(account_id,enabled,next_run_at) WHERE deleted_at IS NULL;
             CREATE TABLE IF NOT EXISTS activity_groups (activity_id INTEGER NOT NULL,account_id TEXT NOT NULL,group_id INTEGER NOT NULL,PRIMARY KEY(activity_id,account_id,group_id),FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE,FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS activity_times (activity_id INTEGER NOT NULL,local_time TEXT NOT NULL,PRIMARY KEY(activity_id,local_time),FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE);
             CREATE TABLE IF NOT EXISTS activity_runs (id INTEGER PRIMARY KEY AUTOINCREMENT,activity_id INTEGER NOT NULL,account_id TEXT NOT NULL,group_id INTEGER NOT NULL,scheduled_for TEXT NOT NULL,run_key TEXT NOT NULL UNIQUE,state TEXT NOT NULL DEFAULT 'pending',text TEXT NOT NULL DEFAULT '',content_source TEXT NOT NULL DEFAULT 'fixed',attempts INTEGER NOT NULL DEFAULT 0,next_retry_at TEXT,last_error TEXT NOT NULL DEFAULT '',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,completed_at TEXT,FOREIGN KEY(activity_id) REFERENCES activities(id) ON DELETE CASCADE,FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE);
             CREATE INDEX IF NOT EXISTS activity_runs_history_idx ON activity_runs(account_id,activity_id,scheduled_for DESC,id DESC);",
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute(
            // 旧任务的 reminder_at 是 RFC3339（UTC）。活动表的 start_date/local_time 是
            // timezone 列所指时区的本地墙上时间，所以必须先转成 localtime 再截取，
            // 否则会整体偏移一个 UTC offset。content 截断到 1000 字符、title 必须非空，
            // 保证迁移出来的活动一定能通过 activities::validate。
            "INSERT OR IGNORE INTO activities(account_id,name,content,enabled,ai_optimize,ai_instructions,timezone,start_date,end_date,weekdays_json,next_run_at,source_key,created_at,updated_at) SELECT account_id,TRIM(title),substr(CASE WHEN TRIM(description)='' THEN TRIM(title) ELSE TRIM(description) END,1,1000),0,0,'',?,COALESCE(date(COALESCE(reminder_at,due_at,created_at),'localtime'),substr(COALESCE(reminder_at,due_at,created_at),1,10)),COALESCE(date(COALESCE(reminder_at,due_at,created_at),'localtime'),substr(COALESCE(reminder_at,due_at,created_at),1,10)),'[1,2,3,4,5,6,7]',NULL,'legacy-task:'||id,created_at,updated_at FROM tasks WHERE TRIM(title)<>''",
            params![migration_timezone],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch(
            "INSERT OR IGNORE INTO activity_groups(activity_id,account_id,group_id) SELECT activities.id,tasks.account_id,tasks.group_id FROM tasks JOIN activities ON activities.account_id=tasks.account_id AND activities.source_key='legacy-task:'||tasks.id;
             INSERT OR IGNORE INTO activity_times(activity_id,local_time) SELECT activities.id,COALESCE(strftime('%H:%M',tasks.reminder_at,'localtime'),'09:00') FROM tasks JOIN activities ON activities.account_id=tasks.account_id AND activities.source_key='legacy-task:'||tasks.id;",
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    // 旧的 reminder_loop 已经被 activity_loop 取代，任何仍然挂着的任务提醒如果只迁移成
    // 停用草稿就会静默失效。这里把「未发送且未结束」的提醒启用，让它继续按原定时刻发出；
    // 已经过期的那些会在 generate_due_activity_runs 里落一条 missed 历史后自然转为不再触发。
    //
    // 只在真正从 <13 升级时执行一次：这个迁移每次启动都会跑，不加版本闸会反复覆盖用户
    // 后来手工停用的活动。
    if old_version < 13 {
        transaction
            .execute(
                "UPDATE activities SET enabled=1,next_run_at=(SELECT tasks.reminder_at FROM tasks WHERE 'legacy-task:'||tasks.id=activities.source_key AND tasks.account_id=activities.account_id),updated_at=? \
                 WHERE activities.source_key LIKE 'legacy-task:%' AND activities.deleted_at IS NULL AND activities.enabled=0 \
                   AND EXISTS (SELECT 1 FROM activity_groups WHERE activity_groups.activity_id=activities.id) \
                   AND EXISTS (SELECT 1 FROM activity_times WHERE activity_times.activity_id=activities.id) \
                   AND EXISTS (SELECT 1 FROM tasks WHERE 'legacy-task:'||tasks.id=activities.source_key AND tasks.account_id=activities.account_id \
                     AND tasks.reminder_at IS NOT NULL AND tasks.reminder_sent_at IS NULL \
                     AND tasks.reminder_state IN ('pending','retry') AND tasks.status<>'done')",
                params![Utc::now().to_rfc3339()],
            )
            .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    }
    transaction
        .execute_batch("CREATE TABLE IF NOT EXISTS gateway_capability_verifications (fingerprint TEXT NOT NULL,capability TEXT NOT NULL,source TEXT NOT NULL,status TEXT NOT NULL,automatic_allowed INTEGER NOT NULL DEFAULT 0,evidence_hash TEXT NOT NULL DEFAULT '',last_error TEXT NOT NULL DEFAULT '',verified_at TEXT NOT NULL,PRIMARY KEY(fingerprint,capability)); CREATE TABLE IF NOT EXISTS ai_provider_endpoints (id INTEGER PRIMARY KEY AUTOINCREMENT,account_id TEXT NOT NULL,name TEXT NOT NULL,base_url TEXT NOT NULL DEFAULT '',webhook_url TEXT NOT NULL DEFAULT '',api_backend TEXT NOT NULL DEFAULT 'chat_completions',model TEXT NOT NULL DEFAULT 'deepseek-v4-pro',secret_ref TEXT NOT NULL DEFAULT '',priority INTEGER NOT NULL DEFAULT 0,enabled INTEGER NOT NULL DEFAULT 1,health_status TEXT NOT NULL DEFAULT 'unchecked',failure_count INTEGER NOT NULL DEFAULT 0,cooldown_until TEXT,last_error TEXT NOT NULL DEFAULT '',last_checked_at TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE); CREATE INDEX IF NOT EXISTS ai_provider_endpoints_account_priority_idx ON ai_provider_endpoints(account_id,enabled DESC,priority,id);")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("CREATE TABLE IF NOT EXISTS expected_member_card_updates (account_id TEXT NOT NULL,group_id INTEGER NOT NULL,user_id INTEGER NOT NULL,card_name TEXT NOT NULL,source_key TEXT NOT NULL,expires_at TEXT NOT NULL,created_at TEXT NOT NULL,PRIMARY KEY(account_id,group_id,user_id,card_name)); CREATE INDEX IF NOT EXISTS expected_member_card_updates_expiry_idx ON expected_member_card_updates(expires_at);")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    add_column_if_missing(
        &transaction,
        "ai_provider_endpoints",
        "api_backend",
        "TEXT NOT NULL DEFAULT 'chat_completions'",
    )
    .map_err(|error| {
        AppError::new(
            "database_migration",
            format!("补齐 ai_provider_endpoints.api_backend 失败：{error}"),
        )
    })?;
    add_column_if_missing(
        &transaction,
        "ai_provider_endpoints",
        "reasoning_effort",
        "TEXT NOT NULL DEFAULT 'low'",
    )
    .map_err(|error| {
        AppError::new(
            "database_migration",
            format!("补齐 ai_provider_endpoints.reasoning_effort 失败：{error}"),
        )
    })?;
    transaction
        .execute(
            "UPDATE ai_provider_endpoints SET api_backend='chat_completions' WHERE api_backend IS NULL OR TRIM(api_backend)=''",
            [],
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    // ── v14：effect_outbox 扩展列 + 索引 + 保留策略表 ──────────────────────────
    // 所有变更均为纯增量，零行为改动；已有行从列默认值获得对应值。
    for (table, column, definition) in [
        ("effect_outbox", "priority",       "INTEGER NOT NULL DEFAULT 100"),
        ("effect_outbox", "lane",           "TEXT NOT NULL DEFAULT 'default'"),
        ("effect_outbox", "order_key",      "TEXT"),
        ("effect_outbox", "correlation_id", "TEXT NOT NULL DEFAULT ''"),
        ("effect_outbox", "origin",         "TEXT NOT NULL DEFAULT 'unknown'"),
        ("effect_outbox", "expires_at",     "TEXT"),
    ] {
        add_column_if_missing(&transaction, table, column, definition).map_err(|error| {
            AppError::new(
                "database_migration",
                format!("补齐 {table}.{column} 失败：{error}"),
            )
        })?;
    }
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS retention_policies (
               table_name  TEXT    NOT NULL PRIMARY KEY,
               max_days    INTEGER NOT NULL DEFAULT 90,
               max_rows    INTEGER NOT NULL DEFAULT 100000,
               batch_size  INTEGER NOT NULL DEFAULT 500,
               enabled     INTEGER NOT NULL DEFAULT 1,
               updated_at  TEXT    NOT NULL
             );
             INSERT OR IGNORE INTO retention_policies(table_name,max_days,max_rows,batch_size,enabled,updated_at) VALUES
               ('messages',        90,  200000, 500, 1, '2026-01-01T00:00:00Z'),
               ('audit_events',   180,  100000, 500, 1, '2026-01-01T00:00:00Z'),
               ('effect_outbox',   30,   50000, 500, 1, '2026-01-01T00:00:00Z'),
               ('gateway_inbox',   14,   50000, 500, 1, '2026-01-01T00:00:00Z'),
               ('rule_evaluations',60,  100000, 500, 1, '2026-01-01T00:00:00Z');
             CREATE INDEX IF NOT EXISTS audit_events_account_time_idx ON audit_events(account_id, created_at DESC, id DESC);
             CREATE INDEX IF NOT EXISTS effect_outbox_claim_idx ON effect_outbox(state, priority, created_at, id);
             CREATE INDEX IF NOT EXISTS effect_outbox_order_key_idx ON effect_outbox(order_key, state) WHERE order_key IS NOT NULL;
             CREATE INDEX IF NOT EXISTS messages_account_group_time_idx ON messages(account_id, group_id, received_at DESC);
             CREATE INDEX IF NOT EXISTS gateway_inbox_account_time_idx ON gateway_inbox(account_id, received_at);
             CREATE INDEX IF NOT EXISTS rule_evaluations_account_time_idx ON rule_evaluations(account_id, created_at);",
        )
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("PRAGMA user_version = 14;")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .commit()
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    Ok(())
}

fn recover_processing_effects(
    connection: &mut Connection,
    reason: &str,
) -> rusqlite::Result<usize> {
    let pending = {
        let mut statement = connection.prepare(
            "SELECT id,account_id,group_id,effect_type,payload_json,dedupe_key FROM effect_outbox WHERE state='processing' ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if pending.is_empty() {
        return Ok(0);
    }
    let receipt_json = serde_json::json!({
        "status": "unknown",
        "businessMessage": reason,
    })
    .to_string();
    let now = Utc::now().to_rfc3339();
    let transaction = connection.transaction()?;
    for (id, account_id, group_id, effect_type, payload_json, dedupe_key) in &pending {
        let payload: Value = serde_json::from_str(payload_json).unwrap_or(Value::Null);
        let user_id = payload.get("userId").and_then(Value::as_i64).unwrap_or(0);
        transaction.execute(
            "UPDATE effect_outbox SET state='unknown',next_attempt_at=NULL,claimed_at=NULL,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
            params![reason, receipt_json, id],
        )?;
        if payload
            .get("recordAction")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let kind = payload
                .get("actionKind")
                .and_then(Value::as_str)
                .unwrap_or(effect_type);
            transaction.execute(
                "INSERT INTO actions(account_id,group_id,user_id,message_id,rule_id,kind,mode,duration_seconds,reason,success,error,receipt_json,dedupe_key,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,dedupe_key) WHERE dedupe_key<>'' DO UPDATE SET success=excluded.success,error=excluded.error,receipt_json=excluded.receipt_json,reason=excluded.reason,created_at=excluded.created_at",
                params![account_id,group_id,user_id,payload.get("messageId").and_then(Value::as_i64),payload.get("ruleId").and_then(Value::as_i64),kind,payload.get("mode").and_then(Value::as_str).unwrap_or("automatic"),payload.get("durationSeconds").and_then(Value::as_i64).unwrap_or(0),payload.get("reason").and_then(Value::as_str).unwrap_or_default(),0,reason,receipt_json,dedupe_key,now],
            )?;
        }
        let details = serde_json::json!({
            "effect": effect_type,
            "success": false,
            "status": "unknown",
            "error": reason,
            "receipt": {
                "status": "unknown",
                "businessMessage": reason,
            },
        })
        .to_string();
        transaction.execute(
            "INSERT INTO audit_events(account_id,group_id,user_id,actor,event,level,details,created_at) VALUES(?,?,?,?,?,?,?,?)",
            params![account_id,group_id,user_id,"DH BOT","effect_recovered_unknown","warning",details,now],
        )?;
    }
    transaction.commit()?;
    Ok(pending.len())
}

impl Database {
    fn runtime_backlog(&self) -> AppResult<Vec<RuntimeBacklog>> {
        self.with_connection(|connection| {
            let mut backlog = Vec::new();
            let mut messages = connection.prepare(
                "SELECT account_id,group_id,COUNT(*) FROM messages WHERE acknowledged_at IS NOT NULL AND processed_at IS NULL AND processing_state IN ('pending','queued','retry','processing') GROUP BY account_id,group_id",
            )?;
            backlog.extend(
                messages
                    .query_map([], |row| {
                        Ok(RuntimeBacklog {
                            account_id: row.get(0)?,
                            group_id: row.get(1)?,
                            lane: "message".into(),
                            detail: String::new(),
                            count: row.get::<_, i64>(2)?.max(0) as usize,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?,
            );
            let mut effects = connection.prepare(
                "SELECT account_id,group_id,effect_type,COUNT(*) FROM effect_outbox WHERE state IN ('queued','retry','processing') GROUP BY account_id,group_id,effect_type",
            )?;
            backlog.extend(
                effects
                    .query_map([], |row| {
                        Ok(RuntimeBacklog {
                            account_id: row.get(0)?,
                            group_id: row.get(1)?,
                            lane: "write".into(),
                            detail: row.get(2)?,
                            count: row.get::<_, i64>(3)?.max(0) as usize,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?,
            );
            let mut cards = connection.prepare(
                "SELECT account_id,group_id,COUNT(*) FROM card_rename_jobs WHERE state IN ('queued','retry','processing') GROUP BY account_id,group_id",
            )?;
            backlog.extend(
                cards
                    .query_map([], |row| {
                        Ok(RuntimeBacklog {
                            account_id: row.get(0)?,
                            group_id: row.get(1)?,
                            lane: "cardRename".into(),
                            detail: String::new(),
                            count: row.get::<_, i64>(2)?.max(0) as usize,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?,
            );
            Ok(backlog)
        })
        .map_err(|error| AppError::new("runtime_backlog", error.to_string()))
    }

    pub fn open(paths: &AppPaths) -> AppResult<Self> {
        if let Some(parent) = paths.database.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::new("database_open", error.to_string()))?;
        }
        let database_existed = paths.database.exists();
        let mut connection = Connection::open(&paths.database).map_err(|error| {
            AppError::new("database_open", format!("打开 3.0 数据库失败：{error}"))
        })?;
        if database_existed {
            if let Err(mut error) = quick_check_connection(&connection) {
                if is_disk_io_error(&error) {
                    match protect_database_files(&paths.database, &paths.v3) {
                        Ok(path) => {
                            error.detail_ref = Some(path.display().to_string());
                            error.message = format!(
                                "{}；原始数据库未修改，已创建保护备份：{}",
                                error.message,
                                path.display()
                            );
                        }
                        Err(snapshot_error) => {
                            error.message = format!(
                                "{}；保护备份失败：{}",
                                error.message, snapshot_error.message
                            );
                        }
                    }
                }
                return Err(error);
            }
        }
        let old_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| AppError::new("database_version", error.to_string()))?;
        if database_existed && old_version < 8 {
            snapshot_before_migration(paths, &connection, old_version)?;
        }
        connection.execute_batch(SCHEMA).map_err(|error| {
            AppError::new("database_schema", format!("初始化 3.0 数据库失败：{error}"))
        })?;
        migrate_schema(&mut connection, old_version)?;
        let database = Self {
            path: paths.database.clone(),
            connection: Arc::new(Mutex::new(connection)),
        };
        database.prepare().map_err(AppError::from)?;
        database.quick_check()?;
        database.ensure_defaults()?;
        Ok(database)
    }

    fn prepare(&self) -> Result<(), InternalError> {
        Ok(self.with_connection(|connection| {
            let now = Utc::now();
            connection.execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
            )?;
            connection.execute(
                "UPDATE messages SET processing_state='pending',next_attempt_at=NULL,last_error='DH BOT 重启后已恢复处理' WHERE processing_state='processing'",
                [],
            )?;
            connection.execute(
                "UPDATE card_rename_jobs SET state='queued',next_attempt_at=NULL,last_error='DH BOT 重启后已恢复排队',updated_at=? WHERE state='processing'",
                params![Utc::now().to_rfc3339()],
            )?;
            connection.execute(
                "UPDATE gateway_inbox SET state='retry',next_attempt_at=NULL,claimed_at=NULL,last_error='DH BOT 重启后已恢复处理' WHERE state='processing'",
                [],
            )?;
            recover_processing_effects(connection, "DH BOT 重启，远端执行结果未知")?;
            connection.execute(
                "UPDATE tasks SET reminder_state='retry',reminder_next_attempt_at=NULL,reminder_claimed_at=NULL,reminder_last_error='DH BOT 重启后已恢复提醒' WHERE reminder_state='processing' AND reminder_sent_at IS NULL",
                [],
            )?;
            connection.execute(
                "UPDATE activity_runs SET state='retry',next_retry_at=NULL,last_error='DH BOT 重启后已恢复活动处理',updated_at=? WHERE state='preparing'",
                params![Utc::now().to_rfc3339()],
            )?;
            connection.execute(
                "UPDATE ai_runs SET state='retry',next_retry_at=NULL,last_error='DH BOT 重启后已恢复执行',updated_at=? WHERE state='processing'",
                params![Utc::now().to_rfc3339()],
            )?;
            connection.execute(
                "UPDATE summary_runs SET state='retry',next_retry_at=NULL,last_error='DH BOT 重启后已恢复执行',updated_at=? WHERE state='processing'",
                params![Utc::now().to_rfc3339()],
            )?;
            // Messages from the pre-ACK pipeline were persisted without a
            // source acknowledgement and must never be replayed on a later
            // launch. Keep them visible as historical records instead of
            // presenting them as actionable pending work.
            connection.execute(
                "UPDATE messages SET processed_at=?,processing_state='ignored',next_attempt_at=NULL,last_error='历史未确认消息已归档，不会自动执行' WHERE acknowledged_at IS NULL AND processed_at IS NULL AND processing_state IN ('pending','queued','retry') AND received_at<?",
                params![now.to_rfc3339(), (now - ChronoDuration::minutes(15)).to_rfc3339()],
            )?;
            reconcile_alias_backed_legacy_members(connection)?;
            Ok(())
        })?)
    }

    pub(crate) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, rusqlite::Error> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        operation(&mut connection)
    }

    pub fn quick_check(&self) -> AppResult<()> {
        let rows = self
            .with_connection(|connection| quick_check_rows(connection))
            .map_err(|error| {
                AppError::new(
                    "database_corrupt",
                    format!("SQLite 完整性检查失败：{error}"),
                )
            })?;
        if rows.len() == 1 && rows[0] == "ok" {
            Ok(())
        } else {
            Err(AppError::new(
                "database_corrupt",
                format!("SQLite 完整性检查失败：{}", rows.join("；")),
            ))
        }
    }

    pub fn status(&self) -> AppResult<DatabaseStatus> {
        let status = self
            .with_connection(|connection| {
                let version: i64 =
                    connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
                let accounts: i64 =
                    connection.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))?;
                let groups: i64 =
                    connection.query_row("SELECT COUNT(*) FROM groups", [], |row| row.get(0))?;
                let messages: i64 =
                    connection.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;

                // Queue depth per state
                let mut stmt = connection.prepare(
                    "SELECT state, COUNT(*) FROM effect_outbox GROUP BY state ORDER BY state",
                )?;
                let queue_depth: Vec<QueueDepthRow> = stmt
                    .query_map([], |row| {
                        Ok(QueueDepthRow { state: row.get(0)?, count: row.get(1)? })
                    })?
                    .filter_map(Result::ok)
                    .collect();

                // Retention policy rows (may not exist in pre-v14 schemas)
                let retention_policies: Vec<RetentionPolicyRow> = connection
                    .prepare(
                        "SELECT table_name,max_days,max_rows,batch_size,enabled FROM retention_policies ORDER BY table_name",
                    )
                    .and_then(|mut stmt| {
                        let rows = stmt
                            .query_map([], |row| {
                                let enabled: i64 = row.get(4)?;
                                Ok(RetentionPolicyRow {
                                    table_name: row.get(0)?,
                                    max_days: row.get(1)?,
                                    max_rows: row.get(2)?,
                                    batch_size: row.get(3)?,
                                    enabled: enabled != 0,
                                })
                            })?
                            .filter_map(Result::ok)
                            .collect();
                        Ok(rows)
                    })
                    .unwrap_or_default();

                // Integrity check capped at 16 errors so the report stays compact
                let index_integrity: String = connection
                    .query_row("PRAGMA integrity_check(16)", [], |row| row.get(0))
                    .unwrap_or_else(|_| "error".into());

                Ok(DatabaseStatus {
                    path: self.path.display().to_string(),
                    schema_version: version,
                    integrity: "ok".into(),
                    accounts,
                    groups,
                    messages,
                    queue_depth,
                    retention_policies,
                    index_integrity,
                })
            })
            .map_err(InternalError::from)?;
        Ok(status)
    }

    fn protect_snapshot_after_io_error(&self) -> AppResult<PathBuf> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppError::new("database_backup", "SQLite 数据库目录不可用"))?;
        let directory = parent
            .join("backups")
            .join(format!("dh-io-{}", Utc::now().format("%Y%m%d-%H%M%S-%3f")));
        fs::create_dir_all(&directory)
            .map_err(|error| AppError::new("database_backup", error.to_string()))?;
        let mut copied = 0_u8;
        for suffix in ["", "-wal", "-shm"] {
            let source = PathBuf::from(format!("{}{}", self.path.display(), suffix));
            if !source.exists() {
                continue;
            }
            let target = directory.join(
                source
                    .file_name()
                    .ok_or_else(|| AppError::new("database_backup", "SQLite 文件名不可用"))?,
            );
            fs::copy(&source, &target)
                .map_err(|error| AppError::new("database_backup", error.to_string()))?;
            copied = copied.saturating_add(1);
        }
        if copied == 0 {
            return Err(AppError::new(
                "database_backup",
                "未找到可复制的 SQLite 文件",
            ));
        }
        Ok(directory)
    }

    fn ensure_defaults(&self) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute("INSERT OR IGNORE INTO app_settings(key,value,sensitive,updated_at) VALUES('ai.model','deepseek-v4-pro',0,?)", params![now])?;
            connection.execute("INSERT OR IGNORE INTO app_settings(key,value,sensitive,updated_at) VALUES('ai.base_url','',0,?)", params![now])?;
            connection.execute("INSERT OR IGNORE INTO app_settings(key,value,sensitive,updated_at) VALUES('ai.webhook_url','',0,?)", params![now])?;
            connection.execute("INSERT OR IGNORE INTO app_settings(key,value,sensitive,updated_at) VALUES('automation.mode','observe',0,?)", params![now])?;
            Ok(())
        }).map_err(|error| AppError::new("database_defaults", error.to_string()))
    }

    /// Adds the reviewed starter rules and knowledge base once per account.
    /// Existing rows are intentionally left untouched so an administrator's
    /// edits survive upgrades and repeated logins.
    pub fn ensure_account_defaults(&self, account_id: &str) -> AppResult<()> {
        let marker_key = format!("defaults.content.version.2.{account_id}");
        let result = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let now = Utc::now().to_rfc3339();
            transaction.execute(
                "INSERT OR IGNORE INTO business_apps(account_id,app_id,name,description,version,enabled,status,status_detail,last_checked_at,updated_at) VALUES(?,'prediction','预测','读取公开或官方开奖并生成统计参考','1.0.0',0,'unchecked','尚未检查数据源',NULL,?)",
                params![account_id, now],
            )?;
            if transaction
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?",
                    params![marker_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                == Some("ready")
            {
                return Ok(());
            }

            for rule in defaults::default_rules(account_id) {
                let exists: Option<i64> = transaction
                    .query_row(
                        "SELECT id FROM rules WHERE account_id=? AND group_id=0 AND name=?",
                        params![account_id, rule.name],
                        |row| row.get(0),
                    )
                    .optional()?;
                if exists.is_some() {
                    continue;
                }
                let roles = serde_json::to_string(&rule.exempt_roles).unwrap_or_else(|_| "[]".into());
                let users = serde_json::to_string(&rule.exempt_user_ids).unwrap_or_else(|_| "[]".into());
                transaction.execute(
                    "INSERT INTO rules(account_id,group_id,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![account_id,rule.group_id,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,rule.cooldown_seconds,rule.priority,rule.mode,0_i64,rule.semantic_threshold,roles,users,now,now],
                )?;
                let rule_id = transaction.last_insert_rowid();
                for (position, action) in rule.actions.iter().enumerate() {
                    transaction.execute(
                        "INSERT INTO rule_actions(rule_id,kind,duration_seconds,message,position) VALUES(?,?,?,?,?)",
                        params![rule_id, action.kind, action.duration_seconds, action.message, position as i64],
                    )?;
                }
            }

            let built_in_base: Option<(i64, i64)> = transaction
                .query_row(
                    "SELECT id,built_in FROM knowledge_bases WHERE account_id=? AND name=?",
                    params![account_id, defaults::DEFAULT_KNOWLEDGE_BASE_NAME],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let base_id = if let Some((id, _)) = built_in_base {
                id
            } else {
                transaction.execute(
                    "INSERT OR IGNORE INTO knowledge_bases(account_id,name,description,enabled,built_in,read_only,created_at,updated_at) VALUES(?,?,?,1,1,1,?,?)",
                    params![account_id, defaults::DEFAULT_KNOWLEDGE_BASE_NAME, defaults::DEFAULT_KNOWLEDGE_BASE_DESCRIPTION, now, now],
                )?;
                transaction.query_row(
                    "SELECT id FROM knowledge_bases WHERE account_id=? AND name=?",
                    params![account_id, defaults::DEFAULT_KNOWLEDGE_BASE_NAME],
                    |row| row.get(0),
                )?
            };

            let is_built_in: i64 = transaction.query_row(
                "SELECT built_in FROM knowledge_bases WHERE id=? AND account_id=?",
                params![base_id, account_id],
                |row| row.get(0),
            )?;
            if is_built_in != 0 {
                for document in defaults::DEFAULT_KNOWLEDGE_DOCUMENTS {
                    let digest = Sha256::digest(document.content.as_bytes());
                    let content_hash = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                    transaction.execute(
                        "INSERT OR IGNORE INTO knowledge_documents(base_id,title,kind,content,source,content_hash,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,1,?,?)",
                        params![base_id, document.title, "markdown", document.content, "built-in", content_hash, now, now],
                    )?;
                }
            }
            transaction.execute(
                "INSERT INTO app_settings(key,value,sensitive,updated_at) VALUES(?,?,0,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
                params![marker_key, "ready", now],
            )?;
            transaction.commit()?;
            Ok(())
        });
        if let Err(error) = &result {
            let _ = self.with_connection(|connection| {
                connection.execute(
                    "INSERT INTO audit_events(account_id,group_id,user_id,actor,event,level,details,created_at) VALUES(?,0,0,'DH BOT','default_content_init_failed','error',?,?)",
                    params![account_id, format!("默认规则与知识库初始化失败：{error}"), Utc::now().to_rfc3339()],
                )?;
                Ok(())
            });
        }
        result.map_err(|error| AppError::new("database_account_defaults", error.to_string()))
    }

    pub fn ensure_business_apps(&self, account_id: &str) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO business_apps(account_id,app_id,name,description,version,enabled,status,status_detail,last_checked_at,updated_at) VALUES(?,'prediction','预测','读取公开或官方开奖并生成统计参考','1.0.0',0,'unchecked','尚未检查数据源',NULL,?)",
                params![account_id, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("business_app_defaults", error.to_string()))
    }

    pub fn list_business_apps(&self, account_id: &str) -> AppResult<Vec<BusinessAppRecord>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT account_id,app_id,name,description,version,enabled,status,status_detail,last_checked_at,updated_at FROM business_apps WHERE account_id=? ORDER BY app_id",
            )?;
            let rows = statement
                .query_map(params![account_id], |row| {
                    Ok(BusinessAppRecord {
                        account_id: row.get(0)?,
                        app_id: row.get(1)?,
                        name: row.get(2)?,
                        description: row.get(3)?,
                        version: row.get(4)?,
                        enabled: row.get::<_, i64>(5)? != 0,
                        status: row.get(6)?,
                        status_detail: row.get(7)?,
                        last_checked_at: optional_time(row.get(8)?),
                        updated_at: parse_time(row.get(9)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>();
            rows
        })
        .map_err(|error| AppError::new("business_apps_read", error.to_string()))
    }

    pub fn set_business_app_enabled(
        &self,
        account_id: &str,
        app_id: &str,
        enabled: bool,
    ) -> AppResult<()> {
        self.ensure_business_apps(account_id)?;
        let changed = self
            .with_connection(|connection| {
                connection.execute(
                "UPDATE business_apps SET enabled=?,updated_at=? WHERE account_id=? AND app_id=?",
                params![bool_i(enabled), Utc::now().to_rfc3339(), account_id, app_id],
            )
            })
            .map_err(|error| AppError::new("business_app_write", error.to_string()))?;
        if changed == 0 {
            return Err(AppError::new(
                "business_app_missing",
                "没有找到指定业务应用",
            ));
        }
        Ok(())
    }

    pub fn business_app_enabled(&self, account_id: &str, app_id: &str) -> AppResult<bool> {
        self.ensure_business_apps(account_id)?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT enabled FROM business_apps WHERE account_id=? AND app_id=?",
                    params![account_id, app_id],
                    |row| Ok(row.get::<_, i64>(0)? != 0),
                )
                .optional()
        })
        .map(|value| value.unwrap_or(false))
        .map_err(|error| AppError::new("business_app_read", error.to_string()))
    }

    pub fn update_business_app_health(
        &self,
        account_id: &str,
        app_id: &str,
        status: &str,
        detail: &str,
        checked_at: DateTime<Utc>,
    ) -> AppResult<()> {
        self.ensure_business_apps(account_id)?;
        self.with_connection(|connection| connection.execute(
            "UPDATE business_apps SET status=?,status_detail=?,last_checked_at=?,updated_at=? WHERE account_id=? AND app_id=?",
            params![status, detail, checked_at.to_rfc3339(), checked_at.to_rfc3339(), account_id, app_id],
        ))
        .map(|_| ())
        .map_err(|error| AppError::new("business_app_health", error.to_string()))
    }

    pub fn claim_business_app_run(&self, run: &BusinessAppRun) -> AppResult<bool> {
        self.ensure_business_apps(&run.account_id)?;
        self.with_connection(|connection| connection.execute(
            "INSERT OR IGNORE INTO business_app_runs(account_id,app_id,group_id,message_id,run_key,status,freshness,ai_used,reply,error,elapsed_ms,created_at,completed_at) VALUES(?,?,?,?,?,'processing','missing',0,'','',0,?,NULL)",
            params![run.account_id,run.app_id,run.group_id,run.message_id,run.run_key,run.created_at.to_rfc3339()],
        ))
        .map(|changed| changed > 0)
        .map_err(|error| AppError::new("business_app_run_claim", error.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_business_app_run(
        &self,
        account_id: &str,
        app_id: &str,
        run_key: &str,
        status: &str,
        freshness: &str,
        ai_used: bool,
        reply: &str,
        error: &str,
        elapsed_ms: i64,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute(
            "UPDATE business_app_runs SET status=?,freshness=?,ai_used=?,reply=?,error=?,elapsed_ms=?,completed_at=? WHERE account_id=? AND app_id=? AND run_key=?",
            params![status,freshness,bool_i(ai_used),reply,error,elapsed_ms,Utc::now().to_rfc3339(),account_id,app_id,run_key],
        ))
        .map(|_| ())
        .map_err(|error| AppError::new("business_app_run_finish", error.to_string()))
    }

    pub fn list_business_app_runs(
        &self,
        account_id: &str,
        app_id: &str,
        limit: usize,
    ) -> AppResult<Vec<BusinessAppRun>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,account_id,app_id,group_id,message_id,run_key,status,freshness,ai_used,reply,error,elapsed_ms,created_at,completed_at FROM business_app_runs WHERE account_id=? AND app_id=? ORDER BY id DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![account_id,app_id,limit.clamp(1,500) as i64], |row| Ok(BusinessAppRun {
                id: row.get(0)?, account_id: row.get(1)?, app_id: row.get(2)?, group_id: row.get(3)?, message_id: row.get(4)?, run_key: row.get(5)?, status: row.get(6)?, freshness: row.get(7)?, ai_used: row.get::<_, i64>(8)? != 0, reply: row.get(9)?, error: row.get(10)?, elapsed_ms: row.get(11)?, created_at: parse_time(row.get(12)?), completed_at: optional_time(row.get(13)?),
            }))?.collect::<Result<Vec<_>, _>>();
            rows
        })
        .map_err(|error| AppError::new("business_app_runs_read", error.to_string()))
    }

    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key=?",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
        })
        .map_err(|error| AppError::new("settings_read", error.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_gateway_capability_verification(
        &self,
        fingerprint: &str,
        capability: &str,
        source: &str,
        status: &str,
        automatic_allowed: bool,
        evidence_hash: &str,
        last_error: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute(
            "INSERT INTO gateway_capability_verifications(fingerprint,capability,source,status,automatic_allowed,evidence_hash,last_error,verified_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(fingerprint,capability) DO UPDATE SET source=excluded.source,status=excluded.status,automatic_allowed=excluded.automatic_allowed,evidence_hash=excluded.evidence_hash,last_error=excluded.last_error,verified_at=excluded.verified_at",
            params![fingerprint, capability, source, status, bool_i(automatic_allowed), evidence_hash, last_error, Utc::now().to_rfc3339()],
        ))
        .map(|_| ())
        .map_err(|error| AppError::new("capability_verification_write", error.to_string()))
    }

    pub fn is_gateway_capability_verified(
        &self,
        fingerprint: &str,
        capability: &str,
    ) -> AppResult<bool> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM gateway_capability_verifications WHERE fingerprint=? AND capability=? AND status='supported' AND automatic_allowed=1)",
                params![fingerprint, capability],
                |row| row.get::<_, i64>(0),
            )
        })
        .map(|value| value != 0)
        .map_err(|error| AppError::new("capability_verification_read", error.to_string()))
    }

    pub fn set_setting(&self, key: &str, value: &str, sensitive: bool) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO app_settings(key,value,sensitive,updated_at) VALUES(?,?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,sensitive=excluded.sensitive,updated_at=excluded.updated_at", params![key, value, if sensitive { 1 } else { 0 }, Utc::now().to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("settings_write", error.to_string()))
    }

    pub fn ensure_ai_provider_endpoints(&self, account_id: &str) -> AppResult<()> {
        self.with_connection(|connection| {
            let exists: i64 = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM ai_provider_endpoints WHERE account_id=?)",
                params![account_id],
                |row| row.get(0),
            )?;
            if exists != 0 {
                return Ok(());
            }
            let setting = |key: &str| -> rusqlite::Result<String> {
                connection
                    .query_row(
                        "SELECT value FROM app_settings WHERE key=?",
                        params![key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map(|value| value.unwrap_or_default())
            };
            let base_url = setting("ai.base_url")?;
            let webhook_url = setting("ai.webhook_url")?;
            let api_backend = {
                let value = setting("ai.api_backend")?;
                if value.trim().is_empty() {
                    "chat_completions".to_string()
                } else {
                    value
                }
            };
            let model = {
                let value = setting("ai.model")?;
                if value.trim().is_empty() {
                    "deepseek-v4-pro".to_string()
                } else {
                    value
                }
            };
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO ai_provider_endpoints(account_id,name,base_url,webhook_url,api_backend,model,secret_ref,priority,enabled,health_status,created_at,updated_at) VALUES(?,?,?,?,? ,?,'ai.api_key',0,1,'unchecked',?,?)",
                params![account_id, "主连接", base_url, webhook_url, api_backend, model, now, now],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("ai_endpoints_initialize", error.to_string()))
    }

    pub fn list_ai_provider_endpoints(
        &self,
        account_id: &str,
    ) -> AppResult<Vec<AiProviderEndpoint>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,account_id,name,base_url,webhook_url,api_backend,model,reasoning_effort,secret_ref,priority,enabled,health_status,failure_count,cooldown_until,last_error,last_checked_at,created_at,updated_at FROM ai_provider_endpoints WHERE account_id=? ORDER BY enabled DESC,priority,id",
            )?;
            let rows = statement
                .query_map(params![account_id], |row| {
                    Ok(AiProviderEndpoint {
                        id: row.get(0)?,
                        account_id: row.get(1)?,
                        name: row.get(2)?,
                        base_url: row.get(3)?,
                        webhook_url: row.get(4)?,
                        api_backend: row.get(5)?,
                        model: row.get(6)?,
                        reasoning_effort: row.get(7)?,
                        secret_ref: row.get(8)?,
                        priority: row.get(9)?,
                        enabled: row.get::<_, i64>(10)? != 0,
                        api_key_configured: false,
                        health_status: row.get(11)?,
                        failure_count: row.get(12)?,
                        cooldown_until: optional_time(row.get(13)?),
                        last_error: row.get(14)?,
                        last_checked_at: optional_time(row.get(15)?),
                        created_at: parse_time(row.get(16)?),
                        updated_at: parse_time(row.get(17)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|error| AppError::new("ai_endpoints_read", error.to_string()))
    }

    pub fn save_ai_provider_endpoint(&self, endpoint: &AiProviderEndpoint) -> AppResult<i64> {
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            if endpoint.id <= 0 {
                connection.execute(
                    "INSERT INTO ai_provider_endpoints(account_id,name,base_url,webhook_url,api_backend,model,reasoning_effort,secret_ref,priority,enabled,health_status,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![endpoint.account_id,endpoint.name,endpoint.base_url,endpoint.webhook_url,endpoint.api_backend,endpoint.model,endpoint.reasoning_effort,endpoint.secret_ref,endpoint.priority,bool_i(endpoint.enabled),endpoint.health_status,now,now],
                )?;
                Ok(connection.last_insert_rowid())
            } else {
                let changed = connection.execute(
                    "UPDATE ai_provider_endpoints SET name=?,base_url=?,webhook_url=?,api_backend=?,model=?,reasoning_effort=?,secret_ref=?,priority=?,enabled=?,health_status=?,updated_at=? WHERE account_id=? AND id=?",
                    params![endpoint.name,endpoint.base_url,endpoint.webhook_url,endpoint.api_backend,endpoint.model,endpoint.reasoning_effort,endpoint.secret_ref,endpoint.priority,bool_i(endpoint.enabled),endpoint.health_status,now,endpoint.account_id,endpoint.id],
                )?;
                if changed == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(endpoint.id)
            }
        })
        .map_err(|error| AppError::new("ai_endpoint_write", error.to_string()))
    }

    pub fn delete_ai_provider_endpoint(&self, account_id: &str, endpoint_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM ai_provider_endpoints WHERE account_id=? AND id=?",
                params![account_id, endpoint_id],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("ai_endpoint_delete", error.to_string()))
    }

    pub fn update_ai_provider_health(
        &self,
        account_id: &str,
        endpoint_id: i64,
        success: bool,
        error: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            let now = Utc::now();
            let previous_failures = connection
                .query_row(
                    "SELECT failure_count FROM ai_provider_endpoints WHERE account_id=? AND id=?",
                    params![account_id, endpoint_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            let failure_count = if success {
                0
            } else {
                previous_failures.saturating_add(1)
            };
            let cooldown_until = if success {
                None
            } else {
                let seconds = if error.contains("401") || error.contains("403") {
                    30 * 60
                } else {
                    30_i64.saturating_mul(1_i64 << (failure_count.saturating_sub(1).min(4)))
                };
                Some(now + chrono::Duration::seconds(seconds))
            };
            connection.execute(
                "UPDATE ai_provider_endpoints SET health_status=?,failure_count=?,cooldown_until=?,last_error=?,last_checked_at=?,updated_at=? WHERE account_id=? AND id=?",
                params![if success { "healthy" } else { "unavailable" },failure_count,cooldown_until.map(|value| value.to_rfc3339()),if success { "" } else { error },now.to_rfc3339(),now.to_rfc3339(),account_id,endpoint_id],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("ai_endpoint_health", error.to_string()))
    }

    pub fn upsert_account(&self, account: &Account) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO accounts(id,display_name,role,discovered_at,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,role=excluded.role,updated_at=excluded.updated_at", params![account.id, account.display_name, account.role, account.discovered_at.to_rfc3339(), account.updated_at.to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("account_write", error.to_string()))
    }

    pub fn upsert_group(&self, group: &Group) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO groups(account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,machine_rules_enabled,ai_rules_enabled,manual_takeover,welcome_message,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET name=excluded.name,owner_user_id=excluded.owner_user_id,updated_at=excluded.updated_at", params![group.account_id, group.group_id, group.name, group.owner_user_id, bool_i(group.enabled), bool_i(group.ai_enabled), bool_i(group.moderation_enabled), bool_i(group.machine_rules_enabled), bool_i(group.ai_rules_enabled), bool_i(group.manual_takeover), group.welcome_message, group.updated_at.to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("group_write", error.to_string()))
    }

    pub fn list_groups(&self, account_id: Option<&str>) -> AppResult<Vec<Group>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,machine_rules_enabled,ai_rules_enabled,manual_takeover,welcome_message,updated_at FROM groups WHERE (?1 IS NULL OR account_id=?1) ORDER BY name,group_id")?;
            let rows = statement.query_map(params![account_id], group_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("groups_read", error.to_string()))
    }

    fn get_group(&self, account_id: &str, group_id: i64) -> AppResult<Option<Group>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,machine_rules_enabled,ai_rules_enabled,manual_takeover,welcome_message,updated_at FROM groups WHERE account_id=? AND group_id=?",
                    params![account_id, group_id],
                    group_from_row,
                )
                .optional()
        })
        .map_err(|error| AppError::new("group_read", error.to_string()))
    }

    pub fn upsert_member(&self, member: &Member) -> AppResult<i64> {
        self.upsert_member_from_wire(member)
    }

    pub fn upsert_member_from_wire(&self, member: &Member) -> AppResult<i64> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let effective_user_id = resolve_member_identity(&transaction, member)?;
            transaction.execute(
                "INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,user_id) DO UPDATE SET nim_id=CASE WHEN excluded.nim_id<>'' THEN excluded.nim_id ELSE members.nim_id END,nickname=CASE WHEN excluded.nickname<>'' THEN excluded.nickname ELSE members.nickname END,card_name=CASE WHEN excluded.card_name<>'' THEN excluded.card_name ELSE members.card_name END,role=excluded.role,account_state=excluded.account_state,present=excluded.present,join_source=CASE WHEN members.join_source='baseline' AND excluded.join_source<>'baseline' THEN excluded.join_source ELSE members.join_source END,joined_at=COALESCE(members.joined_at,excluded.joined_at),last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",
                params![member.account_id,member.group_id,effective_user_id,member.nim_id,member.nickname,member.card_name,member.original_card_name,member.managed_card_name,member.card_suffix,member.role,member.account_state,bool_i(member.blacklisted),bool_i(member.present),member.join_source,bool_i(member.prompt_read),member.locked_card_name,member.violation_count,member.discovered_at.to_rfc3339(),member.joined_at.map(|value| value.to_rfc3339()),member.last_seen_at.to_rfc3339(),member.updated_at.to_rfc3339()],
            )?;
            transaction.commit()?;
            Ok(effective_user_id)
        })
        .map_err(|error| AppError::new("member_write", error.to_string()))
    }

    pub fn resolve_member_user_id(
        &self,
        account_id: &str,
        group_id: i64,
        nim_id: &str,
    ) -> AppResult<Option<i64>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT user_id FROM member_identity_aliases WHERE account_id=? AND group_id=? AND nim_id=?",
                    params![account_id, group_id, nim_id],
                    |row| row.get(0),
                )
                .optional()
        })
        .map_err(|error| AppError::new("member_identity_read", error.to_string()))
    }

    pub fn list_members(&self, account_id: &str, group_id: i64) -> AppResult<Vec<Member>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? ORDER BY nickname,user_id")?;
            let rows = statement.query_map(params![account_id, group_id], member_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("members_read", error.to_string()))
    }

    fn get_member(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<Option<Member>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND user_id=?",
                    params![account_id, group_id, user_id],
                    member_from_row,
                )
                .optional()
        })
        .map_err(|error| AppError::new("member_read", error.to_string()))
    }

    pub fn mark_members_not_present(
        &self,
        account_id: &str,
        group_id: i64,
        present_user_ids: &[i64],
    ) -> AppResult<Vec<Member>> {
        self.mark_members_not_present_before(account_id, group_id, present_user_ids, &Utc::now())
    }

    pub fn mark_members_not_present_before(
        &self,
        account_id: &str,
        group_id: i64,
        present_user_ids: &[i64],
        before: &DateTime<Utc>,
    ) -> AppResult<Vec<Member>> {
        let now = Utc::now().to_rfc3339();
        let before = before.to_rfc3339();
        self.with_connection(|connection| {
            let current = {
                let mut statement = connection.prepare("SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND present=1 AND updated_at<=?")?;
                let result = statement.query_map(params![account_id, group_id, before], member_from_row)?.collect::<Result<Vec<_>, _>>()?;
                result
            };
            for member in &current {
                if !present_user_ids.contains(&member.user_id) {
                    connection.execute("UPDATE members SET present=0,updated_at=? WHERE account_id=? AND group_id=? AND user_id=? AND updated_at<=?", params![now,account_id,group_id,member.user_id,before])?;
                }
            }
            Ok(current.into_iter().filter(|member| !present_user_ids.contains(&member.user_id)).collect())
        }).map_err(|error| AppError::new("members_lifecycle", error.to_string()))
    }

    pub fn mark_member_not_present(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("UPDATE members SET present=0,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?", params![Utc::now().to_rfc3339(), account_id, group_id, user_id]))
            .map(|_| ()).map_err(|error| AppError::new("member_left", error.to_string()))
    }

    pub fn increment_member_violation(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
    ) -> AppResult<i64> {
        self.with_connection(|connection| {
            connection.execute("UPDATE members SET violation_count=violation_count+1,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?", params![Utc::now().to_rfc3339(),account_id,group_id,user_id])?;
            connection.query_row("SELECT violation_count FROM members WHERE account_id=? AND group_id=? AND user_id=?", params![account_id,group_id,user_id], |row| row.get(0))
        }).map_err(|error| AppError::new("member_violation", error.to_string()))
    }

    pub fn expect_member_card_update(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        card_name: &str,
        source_key: &str,
    ) -> AppResult<()> {
        let now = Utc::now();
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM expected_member_card_updates WHERE expires_at<?",
                params![now.to_rfc3339()],
            )?;
            connection.execute(
                "INSERT INTO expected_member_card_updates(account_id,group_id,user_id,card_name,source_key,expires_at,created_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,user_id,card_name) DO UPDATE SET source_key=excluded.source_key,expires_at=excluded.expires_at,created_at=excluded.created_at",
                params![account_id,group_id,user_id,card_name,source_key,(now + ChronoDuration::minutes(2)).to_rfc3339(),now.to_rfc3339()],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("member_card_expectation", error.to_string()))
    }

    pub fn consume_expected_member_card_update(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        card_name: &str,
    ) -> AppResult<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "DELETE FROM expected_member_card_updates WHERE expires_at<?",
                params![now],
            )?;
            let consumed = transaction.execute(
                "DELETE FROM expected_member_card_updates WHERE account_id=? AND group_id=? AND user_id=? AND card_name=? AND expires_at>=?",
                params![account_id,group_id,user_id,card_name,now],
            )? > 0;
            transaction.commit()?;
            Ok(consumed)
        })
        .map_err(|error| AppError::new("member_card_expectation", error.to_string()))
    }

    pub fn set_member_blacklisted(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        blacklisted: bool,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE members SET blacklisted=?,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?",
                params![bool_i(blacklisted), Utc::now().to_rfc3339(), account_id, group_id, user_id],
            )
        })
        .and_then(|changed| {
            if changed == 1 {
                Ok(())
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
        .map_err(|error| AppError::new("member_blacklist", error.to_string()))
    }

    pub fn enqueue_card_job(
        &self,
        account_id: &str,
        group_id: i64,
        plan: &CardPlan,
        welcome_pending: bool,
    ) -> AppResult<bool> {
        if plan.member.user_id == 0 || plan.suggested_name.trim().is_empty() {
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT state FROM card_rename_jobs WHERE account_id=? AND group_id=? AND user_id=? AND desired_name=?",
                    params![account_id, group_id, plan.member.user_id, plan.suggested_name],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.as_deref() == Some("succeeded")
                && plan.member.card_name == plan.suggested_name
            {
                transaction.commit()?;
                return Ok(false);
            }
            if existing.is_some() && existing.as_deref() != Some("succeeded") {
                transaction.commit()?;
                return Ok(false);
            }
            if existing.as_deref() == Some("succeeded") {
                transaction.execute(
                    "UPDATE card_rename_jobs SET nim_id=?,original_name=?,suffix=?,state='queued',attempts=0,next_attempt_at=NULL,last_error='',welcome_pending=CASE WHEN welcome_pending=1 OR ? THEN 1 ELSE 0 END,updated_at=? WHERE account_id=? AND group_id=? AND user_id=? AND desired_name=?",
                    params![plan.member.nim_id,plan.original_name,plan.suffix,welcome_pending,now,account_id,group_id,plan.member.user_id,plan.suggested_name],
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO card_rename_jobs(account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'queued',0,NULL,'',?,?,?)",
                    params![account_id,group_id,plan.member.user_id,plan.member.nim_id,plan.original_name,plan.suggested_name,plan.suffix,if welcome_pending { 1 } else { 0 },now,now],
                )?;
            }
            transaction.commit()?;
            Ok(true)
        })
        .map_err(|error| AppError::new("card_job_write", error.to_string()))
    }

    pub fn list_card_jobs(
        &self,
        account_id: &str,
        group_id: i64,
        limit: usize,
    ) -> AppResult<Vec<CardRenameJob>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at FROM card_rename_jobs WHERE account_id=? AND group_id=? ORDER BY CASE state WHEN 'processing' THEN 0 WHEN 'queued' THEN 1 WHEN 'retry' THEN 2 WHEN 'failed' THEN 3 ELSE 4 END,created_at,id LIMIT ?")?;
            let rows = statement.query_map(params![account_id, group_id, limit.clamp(1, 1000) as i64], card_job_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("card_job_read", error.to_string()))
    }

    pub fn claim_next_card_job(&self, account_id: &str) -> AppResult<Option<CardRenameJob>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let id: Option<i64> = transaction
                .query_row(
                    "SELECT id FROM card_rename_jobs WHERE account_id=? AND state IN ('queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?) AND NOT EXISTS (SELECT 1 FROM card_rename_jobs active WHERE active.account_id=card_rename_jobs.account_id AND active.group_id=card_rename_jobs.group_id AND active.state='processing') AND NOT EXISTS (SELECT 1 FROM app_settings paused WHERE paused.key=('card.paused.' || card_rename_jobs.account_id || '.' || card_rename_jobs.group_id) AND paused.value='true') ORDER BY created_at,id LIMIT 1",
                    params![account_id, now],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(id) = id else {
                transaction.commit()?;
                return Ok(None);
            };
            transaction.execute(
                "UPDATE card_rename_jobs SET state='processing',attempts=attempts+1,updated_at=? WHERE id=?",
                params![now, id],
            )?;
            let job = transaction.query_row(
                "SELECT id,account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at FROM card_rename_jobs WHERE id=?",
                params![id],
                card_job_from_row,
            )?;
            transaction.commit()?;
            Ok(Some(job))
        })
        .map_err(|error| AppError::new("card_job_claim", error.to_string()))
    }

    pub fn finish_card_job(
        &self,
        job: &CardRenameJob,
        success: bool,
        retryable: bool,
        error: &str,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if success {
                transaction.execute(
                    "UPDATE card_rename_jobs SET state='succeeded',next_attempt_at=NULL,last_error='',updated_at=? WHERE id=?",
                    params![now, job.id],
                )?;
                // `card_name` is a wire value owned by 旺商聊. Never manufacture a
                // successful readback locally: it is refreshed only by the next
                // HTTP/NIM roster synchronization. This keeps a UI-visible rename
                // failure detectable instead of masking it with the desired value.
                transaction.execute(
                    "UPDATE members SET managed_card_name=?,locked_card_name=?,card_suffix=?,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?",
                    params![job.desired_name,job.desired_name,job.suffix,now,job.account_id,job.group_id,job.user_id],
                )?;
            } else {
                let (state, next_retry) = if !retryable || job.attempts >= 5 {
                    ("failed", None)
                } else {
                    let delay = match job.attempts {
                        1 => 5,
                        2 => 30,
                        3 => 120,
                        4 => 600,
                        _ => 1800,
                    };
                    ("retry", Some((Utc::now() + ChronoDuration::seconds(delay)).to_rfc3339()))
                };
                transaction.execute(
                    "UPDATE card_rename_jobs SET state=?,next_attempt_at=?,last_error=?,updated_at=? WHERE id=?",
                    params![state,next_retry,error,now,job.id],
                )?;
            }
            transaction.commit()
        })
        .map_err(|error| AppError::new("card_job_finish", error.to_string()))
    }

    pub fn mark_card_welcome_sent(&self, job_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE card_rename_jobs SET welcome_pending=0,updated_at=? WHERE id=?",
                params![Utc::now().to_rfc3339(), job_id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("card_job_welcome", error.to_string()))
    }

    pub fn retry_failed_card_jobs(&self, account_id: &str, group_id: i64) -> AppResult<usize> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE card_rename_jobs SET state='queued',attempts=0,next_attempt_at=NULL,last_error='',updated_at=? WHERE account_id=? AND group_id=? AND state='failed'",
                params![Utc::now().to_rfc3339(), account_id, group_id],
            )
        })
        .map_err(|error| AppError::new("card_job_retry", error.to_string()))
    }

    pub fn ingest_gateway_inbox_batch(
        &self,
        events: &[GatewayInboxEvent],
    ) -> AppResult<BatchIngestResult> {
        self.ingest_gateway_batch(events, &[])
            .map(|(result, _)| result)
    }

    pub fn ingest_gateway_batch(
        &self,
        events: &[GatewayInboxEvent],
        messages: &[Message],
    ) -> AppResult<(BatchIngestResult, Vec<PersistedMessage>)> {
        if events.iter().any(|event| event.event_id.trim().is_empty()) {
            return Err(AppError::new(
                "gateway_inbox_event_id",
                "网关事件 ID 不能为空",
            ));
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut inserted = 0;
            for event in events {
                inserted += transaction.execute(
                    "INSERT OR IGNORE INTO gateway_inbox(account_id,bridge_session,bridge_sequence,event_id,event_type,payload_json,state,attempts,next_attempt_at,last_error,received_at,claimed_at,processed_at) VALUES(?,?,?,?,?,?,'pending',0,NULL,'',?,NULL,NULL)",
                    params![event.account_id,event.bridge_session,event.bridge_sequence,event.event_id,event.event_type,event.payload_json,event.received_at.to_rfc3339()],
                )?;
            }
            let mut persisted_messages = Vec::with_capacity(messages.len());
            for message in messages {
                let changed = transaction.execute(
                    "INSERT OR IGNORE INTO messages(account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error,mentions_json,source_kind,flow) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![message.account_id,message.group_id,message.server_message_id,message.sequence,message.user_id,message.sender_name,message.kind,message.text,message.sent_at.to_rfc3339(),message.received_at.to_rfc3339(),message.processed_at.map(|v| v.to_rfc3339()),message.acknowledged_at.map(|v| v.to_rfc3339()),message.processing_state,message.attempts,message.next_attempt_at.map(|value| value.to_rfc3339()),message.last_error,message.mentions_json,message.source_kind,message.flow],
                )?;
                let (id, state): (i64, String) = transaction.query_row(
                    "SELECT id,processing_state FROM messages WHERE account_id=? AND group_id=? AND server_message_id=?",
                    params![message.account_id,message.group_id,message.server_message_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                persisted_messages.push(PersistedMessage {
                    id,
                    inserted: changed == 1,
                    processed: state == "processed",
                });
            }
            transaction.commit()?;
            Ok((
                BatchIngestResult {
                    inserted,
                    duplicates: events.len().saturating_sub(inserted),
                },
                persisted_messages,
            ))
        })
        .map_err(|error| AppError::new("gateway_inbox_ingest", error.to_string()))
    }

    pub fn claim_gateway_inbox(
        &self,
        account_id: Option<&str>,
        limit: usize,
    ) -> AppResult<Vec<GatewayInboxItem>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let ids = {
                let mut statement = transaction.prepare(
                    "SELECT current.id FROM gateway_inbox AS current WHERE (?1 IS NULL OR current.account_id=?1) AND current.state IN ('pending','retry') AND (current.next_attempt_at IS NULL OR current.next_attempt_at<=?2) AND NOT EXISTS (SELECT 1 FROM gateway_inbox AS prior WHERE prior.account_id=current.account_id AND prior.bridge_session=current.bridge_session AND prior.bridge_sequence>0 AND prior.bridge_sequence<current.bridge_sequence AND prior.state NOT IN ('processed','ignored')) ORDER BY current.received_at,current.id LIMIT ?3",
                )?;
                let result = statement
                    .query_map(
                        params![account_id, now, limit.clamp(1, 1000) as i64],
                        |row| row.get::<_, i64>(0),
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                result
            };
            let mut claimed = Vec::with_capacity(ids.len());
            for id in ids {
                let changed = transaction.execute(
                    "UPDATE gateway_inbox SET state='processing',attempts=attempts+1,claimed_at=?,next_attempt_at=NULL WHERE id=? AND state IN ('pending','retry')",
                    params![now, id],
                )?;
                if changed == 1 {
                    claimed.push(transaction.query_row(
                        "SELECT id,account_id,bridge_session,bridge_sequence,event_id,event_type,payload_json,state,attempts,next_attempt_at,last_error,received_at,claimed_at,processed_at FROM gateway_inbox WHERE id=?",
                        params![id],
                        gateway_inbox_from_row,
                    )?);
                }
            }
            transaction.commit()?;
            Ok(claimed)
        })
        .map_err(|error| AppError::new("gateway_inbox_claim", error.to_string()))
    }

    pub fn finish_gateway_inbox(&self, id: i64, success: bool, error: &str) -> AppResult<()> {
        let now = Utc::now();
        self.with_connection(|connection| {
            if success {
                connection.execute(
                    "UPDATE gateway_inbox SET state='processed',processed_at=?,claimed_at=NULL,next_attempt_at=NULL,last_error='' WHERE id=? AND state='processing'",
                    params![now.to_rfc3339(), id],
                )?;
            } else {
                let attempts: i64 = connection
                    .query_row(
                        "SELECT attempts FROM gateway_inbox WHERE id=?",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(1);
                let state = if attempts >= 5 { "failed" } else { "retry" };
                let next_attempt_at = (attempts < 5)
                    .then(|| (now + retry_delay(attempts)).to_rfc3339());
                connection.execute(
                    "UPDATE gateway_inbox SET state=?,claimed_at=NULL,next_attempt_at=?,last_error=? WHERE id=? AND state='processing'",
                    params![state, next_attempt_at, error, id],
                )?;
            }
            Ok(())
        })
        .map_err(|error| AppError::new("gateway_inbox_finish", error.to_string()))
    }

    pub fn ignore_gateway_inbox_event(
        &self,
        account_id: &str,
        event_id: &str,
        reason: &str,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE gateway_inbox SET state='ignored',processed_at=COALESCE(processed_at,?),claimed_at=NULL,next_attempt_at=NULL,last_error=? WHERE account_id=? AND event_id=? AND state<>'processed'",
                params![now, reason, account_id, event_id],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("gateway_inbox_ignore", error.to_string()))
    }

    pub fn mark_gateway_inbox_processed(
        &self,
        account_id: &str,
        event_ids: &[String],
    ) -> AppResult<usize> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut changed = 0;
            for event_id in event_ids {
                changed += transaction.execute(
                    "UPDATE gateway_inbox SET state=CASE WHEN state='ignored' THEN 'ignored' ELSE 'processed' END,processed_at=COALESCE(processed_at,?),claimed_at=NULL,next_attempt_at=NULL,last_error=CASE WHEN state='ignored' THEN last_error ELSE '' END WHERE account_id=? AND event_id=? AND state<>'processed'",
                    params![now, account_id, event_id],
                )?;
            }
            transaction.commit()?;
            Ok(changed)
        })
        .map_err(|error| AppError::new("gateway_inbox_mark_processed", error.to_string()))
    }

    pub fn processed_gateway_event_ids(
        &self,
        account_id: &str,
        event_ids: &[String],
    ) -> AppResult<Vec<String>> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT state FROM gateway_inbox WHERE account_id=? AND event_id=?")?;
            let mut processed = Vec::new();
            for event_id in event_ids {
                let state = statement
                    .query_row(params![account_id, event_id], |row| row.get::<_, String>(0))
                    .optional()?;
                if matches!(state.as_deref(), Some("processed" | "ignored")) {
                    processed.push(event_id.clone());
                }
            }
            Ok(processed)
        })
        .map_err(|error| AppError::new("gateway_inbox_state", error.to_string()))
    }

    pub fn commit_gateway_ack(
        &self,
        account_id: &str,
        event_ids: &[String],
        message_ids: &[i64],
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for message_id in message_ids {
                transaction.execute(
                    "UPDATE messages SET acknowledged_at=COALESCE(acknowledged_at,?) WHERE id=? AND account_id=?",
                    params![now, message_id, account_id],
                )?;
            }
            for event_id in event_ids {
                transaction.execute(
                    "UPDATE gateway_inbox SET state=CASE WHEN state='ignored' THEN 'ignored' ELSE 'processed' END,processed_at=COALESCE(processed_at,?),claimed_at=NULL,next_attempt_at=NULL,last_error=CASE WHEN state='ignored' THEN last_error ELSE '' END WHERE account_id=? AND event_id=?",
                    params![now, account_id, event_id],
                )?;
            }
            transaction.commit()
        })
        .map_err(|error| AppError::new("gateway_ack_commit", error.to_string()))
    }

    pub fn enqueue_effect(&self, request: &EffectOutboxRequest) -> AppResult<EnqueuedEffect> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let changed = connection.execute(
                "INSERT OR IGNORE INTO effect_outbox(account_id,group_id,effect_type,payload_json,dedupe_key,state,attempts,next_attempt_at,last_error,created_at,claimed_at,completed_at) VALUES(?,?,?,?,?,'queued',0,NULL,'',?,NULL,NULL)",
                params![request.account_id,request.group_id,request.effect_type,request.payload_json,request.dedupe_key,now],
            )?;
            let (id, state) = if changed == 1 || request.dedupe_key.is_empty() {
                connection.query_row(
                    "SELECT id,state FROM effect_outbox WHERE id=last_insert_rowid()",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
            } else {
                connection.query_row(
                    "SELECT id,state FROM effect_outbox WHERE account_id=? AND dedupe_key=?",
                    params![request.account_id, request.dedupe_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
            };
            Ok(EnqueuedEffect {
                id,
                inserted: changed == 1,
                state,
            })
        })
        .map_err(|error| AppError::new("effect_outbox_enqueue", error.to_string()))
    }

    pub fn enqueue_activity_effect(
        &self,
        run_id: i64,
        text: &str,
        source: &str,
        request: &EffectOutboxRequest,
    ) -> AppResult<EnqueuedEffect> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "INSERT OR IGNORE INTO effect_outbox(account_id,group_id,effect_type,payload_json,dedupe_key,state,attempts,next_attempt_at,last_error,created_at,claimed_at,completed_at) VALUES(?,?,?,?,?,'queued',0,NULL,'',?,NULL,NULL)",
                params![request.account_id,request.group_id,request.effect_type,request.payload_json,request.dedupe_key,now],
            )?;
            let (id, state) = if changed == 1 || request.dedupe_key.is_empty() {
                transaction.query_row(
                    "SELECT id,state FROM effect_outbox WHERE id=last_insert_rowid()",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
            } else {
                transaction.query_row(
                    "SELECT id,state FROM effect_outbox WHERE account_id=? AND dedupe_key=?",
                    params![request.account_id, request.dedupe_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
            };
            let run_changed = transaction.execute(
                "UPDATE activity_runs SET state='queued',text=?,content_source=?,updated_at=? WHERE id=? AND state='preparing'",
                params![text,source,now,run_id],
            )?;
            if run_changed != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "activity run is not preparing".into(),
                ));
            }
            transaction.commit()?;
            Ok(EnqueuedEffect {
                id,
                inserted: changed == 1,
                state,
            })
        })
        .map_err(|error| AppError::new("activity_effect_enqueue", error.to_string()))
    }

    pub fn claim_effect_outbox(
        &self,
        account_id: Option<&str>,
        limit: usize,
    ) -> AppResult<Vec<EffectOutboxItem>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let ids = {
                let mut statement = transaction.prepare(
                    "SELECT id FROM effect_outbox WHERE (?1 IS NULL OR account_id=?1) AND state IN ('queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?2) ORDER BY priority,created_at,id LIMIT ?3",
                )?;
                let result = statement
                    .query_map(
                        params![account_id, now, limit.clamp(1, 1000) as i64],
                        |row| row.get::<_, i64>(0),
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                result
            };
            let mut claimed = Vec::with_capacity(ids.len());
            for id in ids {
                let changed = transaction.execute(
                    "UPDATE effect_outbox SET state='processing',attempts=attempts+1,claimed_at=?,next_attempt_at=NULL WHERE id=? AND state IN ('queued','retry')",
                    params![now, id],
                )?;
                if changed == 1 {
                    claimed.push(transaction.query_row(
                    "SELECT id,account_id,group_id,effect_type,payload_json,dedupe_key,state,attempts,next_attempt_at,last_error,receipt_json,created_at,claimed_at,completed_at,priority,lane,order_key,correlation_id,origin,expires_at FROM effect_outbox WHERE id=?",
                        params![id],
                        effect_outbox_from_row,
                    )?);
                }
            }
            transaction.commit()?;
            Ok(claimed)
        })
        .map_err(|error| AppError::new("effect_outbox_claim", error.to_string()))
    }

    pub fn finish_effect_outbox(
        &self,
        id: i64,
        success: bool,
        error: &str,
        receipt_json: &str,
    ) -> AppResult<()> {
        let now = Utc::now();
        self.with_connection(|connection| {
            if success {
                connection.execute(
                    "UPDATE effect_outbox SET state='succeeded',completed_at=?,claimed_at=NULL,next_attempt_at=NULL,last_error='',receipt_json=? WHERE id=? AND state='processing'",
                    params![now.to_rfc3339(), receipt_json, id],
                )?;
            } else {
                let attempts: i64 = connection
                    .query_row(
                        "SELECT attempts FROM effect_outbox WHERE id=?",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(1);
                let state = if attempts >= 5 { "failed" } else { "retry" };
                let next_attempt_at = (attempts < 5)
                    .then(|| (now + retry_delay(attempts)).to_rfc3339());
                connection.execute(
                    "UPDATE effect_outbox SET state=?,claimed_at=NULL,next_attempt_at=?,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
                    params![state, next_attempt_at, error, receipt_json, id],
                )?;
            }
            Ok(())
        })
        .map_err(|error| AppError::new("effect_outbox_finish", error.to_string()))
    }

    pub fn finish_effect_outbox_unknown(
        &self,
        id: i64,
        error: &str,
        receipt_json: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE effect_outbox SET state='unknown',claimed_at=NULL,next_attempt_at=NULL,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
                params![error, receipt_json, id],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("effect_outbox_unknown", error.to_string()))
    }

    pub fn mark_processing_effects_unknown(&self, reason: &str) -> AppResult<usize> {
        self.with_connection(|connection| recover_processing_effects(connection, reason))
            .map_err(|error| AppError::new("effect_outbox_shutdown", error.to_string()))
    }

    pub fn retry_effect_outbox(&self, id: i64) -> AppResult<bool> {
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE effect_outbox SET state='queued',attempts=0,next_attempt_at=NULL,claimed_at=NULL,completed_at=NULL,last_error='' WHERE id=? AND state IN ('unknown','failed')",
                params![id],
            )?;
            Ok(changed == 1)
        })
        .map_err(|error| AppError::new("effect_outbox_retry", error.to_string()))
    }

    pub fn insert_message(&self, message: &Message) -> AppResult<PersistedMessage> {
        self.with_connection(|connection| {
            let changed = connection.execute("INSERT OR IGNORE INTO messages(account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error,mentions_json,source_kind,flow) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![message.account_id,message.group_id,message.server_message_id,message.sequence,message.user_id,message.sender_name,message.kind,message.text,message.sent_at.to_rfc3339(),message.received_at.to_rfc3339(),message.processed_at.map(|v| v.to_rfc3339()),message.acknowledged_at.map(|v| v.to_rfc3339()),message.processing_state,message.attempts,message.next_attempt_at.map(|value| value.to_rfc3339()),message.last_error,message.mentions_json,message.source_kind,message.flow])?;
            let (id, state): (i64, String) = connection.query_row(
                "SELECT id,processing_state FROM messages WHERE account_id=? AND group_id=? AND server_message_id=?",
                params![message.account_id,message.group_id,message.server_message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok(PersistedMessage { id, inserted: changed == 1, processed: state == "processed" })
        }).map_err(|error| AppError::new("message_write", error.to_string()))
    }

    pub fn mark_message_acknowledged(&self, id: i64) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE messages SET acknowledged_at=?,processing_state=CASE WHEN processed_at IS NULL THEN 'queued' ELSE processing_state END WHERE id=?",
                params![now, id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("message_ack", error.to_string()))
    }

    /// Claims one message for processing. The conditional update makes a retry
    /// and a duplicate poll safe even when two workers observe the same row.
    pub fn claim_message(&self, id: i64) -> AppResult<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE messages SET processing_state='processing',attempts=attempts+1 WHERE id=? AND processed_at IS NULL AND processing_state IN ('pending','queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?)",
                params![id, now],
            )?;
            Ok(changed == 1)
        })
        .map_err(|error| AppError::new("message_claim", error.to_string()))
    }

    pub fn finish_message_processing(&self, id: i64, success: bool, error: &str) -> AppResult<()> {
        let now = Utc::now();
        self.with_connection(|connection| {
            if success {
                connection.execute(
                    "UPDATE messages SET processed_at=?,processing_state='processed',next_attempt_at=NULL,last_error='' WHERE id=?",
                    params![now.to_rfc3339(), id],
                )?;
            } else {
                let attempts: i64 = connection
                    .query_row("SELECT attempts FROM messages WHERE id=?", params![id], |row| row.get(0))
                    .optional()?
                    .unwrap_or(1);
                let delay = match attempts {
                    0 | 1 => 5,
                    2 => 30,
                    3 => 120,
                    4 => 600,
                    _ => 1800,
                };
                let state = if attempts >= 5 { "failed" } else { "retry" };
                connection.execute(
                    "UPDATE messages SET processing_state=?,next_attempt_at=?,last_error=? WHERE id=?",
                    params![state, (now + ChronoDuration::seconds(delay)).to_rfc3339(), error, id],
                )?;
            }
            Ok(())
        })
        .map_err(|error| AppError::new("message_finish", error.to_string()))
    }

    pub fn list_pending_messages(&self, account_id: &str, limit: usize) -> AppResult<Vec<Message>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error,mentions_json,source_kind,flow FROM messages WHERE account_id=? AND acknowledged_at IS NOT NULL AND processed_at IS NULL AND processing_state IN ('pending','queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?) ORDER BY sequence,id LIMIT ?")?;
            let rows = statement.query_map(params![account_id, Utc::now().to_rfc3339(), limit.clamp(1, 1000) as i64], message_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("messages_pending", error.to_string()))
    }

    pub fn list_knowledge_bases(&self, account_id: &str) -> AppResult<Vec<KnowledgeBase>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,name,description,enabled,built_in,read_only FROM knowledge_bases WHERE account_id=? ORDER BY built_in DESC,name,id")?;
            let rows = statement.query_map(params![account_id], |row| Ok(KnowledgeBase { id: row.get(0)?, account_id: row.get(1)?, name: row.get(2)?, description: row.get(3)?, enabled: row.get::<_, i64>(4)? != 0, built_in: row.get::<_, i64>(5)? != 0, read_only: row.get::<_, i64>(6)? != 0 }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("knowledge_read", error.to_string()))
    }

    pub fn list_knowledge_for_group(
        &self,
        account_id: &str,
        group_id: i64,
    ) -> AppResult<Vec<KnowledgeDocument>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash,d.enabled FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id JOIN knowledge_base_groups g ON g.base_id=b.id AND g.account_id=b.account_id WHERE b.account_id=? AND g.group_id=? AND b.enabled=1 AND d.enabled=1 AND g.enabled=1 ORDER BY b.name,d.title,d.id")?;
            let rows = statement.query_map(params![account_id, group_id], |row| Ok(KnowledgeDocument { id: row.get(0)?, base_id: row.get(1)?, base_name: row.get(2)?, title: row.get(3)?, kind: row.get(4)?, content: row.get(5)?, source: row.get(6)?, content_hash: row.get(7)?, enabled: row.get::<_, i64>(8)? != 0 }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("knowledge_read", error.to_string()))
    }
}

fn retry_delay(attempts: i64) -> ChronoDuration {
    ChronoDuration::seconds(match attempts {
        0 | 1 => 5,
        2 => 30,
        3 => 120,
        4 => 600,
        _ => 1800,
    })
}

fn resolve_member_identity(
    transaction: &rusqlite::Transaction<'_>,
    member: &Member,
) -> rusqlite::Result<i64> {
    if member.nim_id.is_empty() {
        return Ok(member.user_id);
    }
    let existing_alias: Option<(i64, Option<i64>)> = transaction
        .query_row(
            "SELECT user_id,provisional_user_id FROM member_identity_aliases WHERE account_id=? AND group_id=? AND nim_id=?",
            params![member.account_id, member.group_id, member.nim_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let mut effective_user_id = member.user_id;
    let mut provisional_user_id = None;
    if member.user_id > 0 {
        if let Some(legacy_user_id) = legacy_nim_user_candidate(
            transaction,
            member,
            existing_alias.as_ref().map(|(user_id, _)| *user_id),
        )? {
            merge_member_identity(
                transaction,
                &member.account_id,
                member.group_id,
                legacy_user_id,
                member.user_id,
            )?;
        }
    }
    if member.user_id < 0 {
        if let Some((canonical_user_id, _)) = existing_alias {
            if canonical_user_id > 0 {
                effective_user_id = canonical_user_id;
                provisional_user_id = Some(member.user_id);
            }
        }
    } else if member.user_id > 0 {
        provisional_user_id = existing_alias
            .and_then(|(user_id, provisional)| {
                if user_id < 0 {
                    Some(user_id)
                } else {
                    provisional.filter(|value| *value < 0)
                }
            })
            .or_else(|| {
                transaction
                    .query_row(
                        "SELECT user_id FROM members WHERE account_id=? AND group_id=? AND nim_id=? AND user_id<0 ORDER BY user_id LIMIT 1",
                        params![member.account_id, member.group_id, member.nim_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            });
        if let Some(provisional_user_id) = provisional_user_id {
            if provisional_user_id != member.user_id {
                merge_member_identity(
                    transaction,
                    &member.account_id,
                    member.group_id,
                    provisional_user_id,
                    member.user_id,
                )?;
            }
        }
    }

    let now = member.updated_at.to_rfc3339();
    transaction.execute(
        "INSERT INTO member_identity_aliases(account_id,group_id,nim_id,user_id,provisional_user_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,nim_id) DO UPDATE SET user_id=excluded.user_id,provisional_user_id=COALESCE(member_identity_aliases.provisional_user_id,excluded.provisional_user_id),updated_at=excluded.updated_at",
        params![member.account_id,member.group_id,member.nim_id,effective_user_id,provisional_user_id,now,now],
    )?;
    Ok(effective_user_id)
}

fn legacy_nim_user_candidate(
    transaction: &rusqlite::Transaction<'_>,
    member: &Member,
    aliased_user_id: Option<i64>,
) -> rusqlite::Result<Option<i64>> {
    let Ok(legacy_user_id) = member.nim_id.parse::<i64>() else {
        return Ok(None);
    };
    if legacy_user_id <= 0 || legacy_user_id == member.user_id {
        return Ok(None);
    }
    let legacy: Option<(String, String)> = transaction
        .query_row(
            "SELECT nickname,card_name FROM members WHERE account_id=? AND group_id=? AND user_id=? AND nim_id=''",
            params![member.account_id, member.group_id, legacy_user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((nickname, card_name)) = legacy else {
        return Ok(None);
    };
    let alias_confirms_identity = aliased_user_id == Some(member.user_id);
    let legacy_looks_internal = [nickname.as_str(), card_name.as_str()]
        .iter()
        .all(|value| value.trim().is_empty() || is_internal_member_hash(value));
    Ok((alias_confirms_identity || legacy_looks_internal).then_some(legacy_user_id))
}

fn is_internal_member_hash(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn merge_member_identity(
    transaction: &rusqlite::Transaction<'_>,
    account_id: &str,
    group_id: i64,
    old_user_id: i64,
    real_user_id: i64,
) -> rusqlite::Result<()> {
    if old_user_id == real_user_id {
        return Ok(());
    }
    let provisional: Option<Member> = transaction
        .query_row(
            "SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND user_id=?",
            params![account_id, group_id, old_user_id],
            member_from_row,
        )
        .optional()?;
    let Some(provisional) = provisional else {
        return Ok(());
    };
    let real_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM members WHERE account_id=? AND group_id=? AND user_id=?)",
        params![account_id, group_id, real_user_id],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if !real_exists {
        transaction.execute(
            "INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at) SELECT account_id,group_id,?,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND user_id=?",
            params![real_user_id, account_id, group_id, old_user_id],
        )?;
    } else {
        transaction.execute(
            "UPDATE members SET original_card_name=CASE WHEN original_card_name='' THEN ? ELSE original_card_name END,managed_card_name=CASE WHEN managed_card_name='' THEN ? ELSE managed_card_name END,card_suffix=CASE WHEN card_suffix='' THEN ? ELSE card_suffix END,blacklisted=CASE WHEN blacklisted=1 OR ?=1 THEN 1 ELSE 0 END,join_source=CASE WHEN join_source='baseline' AND ?<>'baseline' THEN ? ELSE join_source END,prompt_read=CASE WHEN prompt_read=0 OR ?=0 THEN 0 ELSE 1 END,locked_card_name=CASE WHEN locked_card_name='' THEN ? ELSE locked_card_name END,violation_count=MAX(violation_count,?),discovered_at=CASE WHEN discovered_at='' OR (?<>'' AND ?<discovered_at) THEN ? ELSE discovered_at END,joined_at=CASE WHEN joined_at IS NULL OR (? IS NOT NULL AND ?<joined_at) THEN ? ELSE joined_at END WHERE account_id=? AND group_id=? AND user_id=?",
            params![provisional.original_card_name,provisional.managed_card_name,provisional.card_suffix,bool_i(provisional.blacklisted),provisional.join_source,provisional.join_source,bool_i(provisional.prompt_read),provisional.locked_card_name,provisional.violation_count,provisional.discovered_at.to_rfc3339(),provisional.discovered_at.to_rfc3339(),provisional.discovered_at.to_rfc3339(),provisional.joined_at.map(|value| value.to_rfc3339()),provisional.joined_at.map(|value| value.to_rfc3339()),provisional.joined_at.map(|value| value.to_rfc3339()),account_id,group_id,real_user_id],
        )?;
    }

    transaction.execute(
        "UPDATE messages SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE actions SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE audit_events SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE rule_evaluations SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE tasks SET assignee_id=? WHERE account_id=? AND group_id=? AND assignee_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE tasks SET created_by=? WHERE account_id=? AND group_id=? AND created_by=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "INSERT INTO rule_runtime_state(rule_id,account_id,group_id,user_id,window_started_at,window_count,last_executed_at,updated_at) SELECT rule_id,account_id,group_id,?,window_started_at,window_count,last_executed_at,updated_at FROM rule_runtime_state WHERE account_id=? AND group_id=? AND user_id=? AND true ON CONFLICT(rule_id,account_id,group_id,user_id) DO UPDATE SET window_started_at=CASE WHEN excluded.window_started_at IS NULL THEN rule_runtime_state.window_started_at WHEN rule_runtime_state.window_started_at IS NULL OR excluded.window_started_at<rule_runtime_state.window_started_at THEN excluded.window_started_at ELSE rule_runtime_state.window_started_at END,window_count=MAX(rule_runtime_state.window_count,excluded.window_count),last_executed_at=CASE WHEN excluded.last_executed_at IS NULL THEN rule_runtime_state.last_executed_at WHEN rule_runtime_state.last_executed_at IS NULL OR excluded.last_executed_at>rule_runtime_state.last_executed_at THEN excluded.last_executed_at ELSE rule_runtime_state.last_executed_at END,updated_at=MAX(rule_runtime_state.updated_at,excluded.updated_at)",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "DELETE FROM rule_runtime_state WHERE account_id=? AND group_id=? AND user_id=?",
        params![account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO rule_whitelist_members(rule_id,user_id) SELECT id,? FROM rules WHERE account_id=? AND EXISTS(SELECT 1 FROM rule_whitelist_members whitelist WHERE whitelist.rule_id=rules.id AND whitelist.user_id=?)",
        params![real_user_id, account_id, old_user_id],
    )?;
    transaction.execute(
        "DELETE FROM rule_whitelist_members WHERE user_id=? AND rule_id IN (SELECT id FROM rules WHERE account_id=?)",
        params![old_user_id, account_id],
    )?;
    transaction.execute(
        "UPDATE effect_outbox SET payload_json=json_set(payload_json,'$.userId',?) WHERE account_id=? AND group_id=? AND state IN ('queued','retry') AND json_valid(payload_json)=1 AND CAST(json_extract(payload_json,'$.userId') AS INTEGER)=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE OR IGNORE card_rename_jobs SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE card_rename_jobs AS target SET nim_id=CASE WHEN target.nim_id='' THEN source.nim_id ELSE target.nim_id END,original_name=CASE WHEN target.original_name='' THEN source.original_name ELSE target.original_name END,suffix=CASE WHEN target.suffix='' THEN source.suffix ELSE target.suffix END,state=CASE WHEN target.state='processing' OR source.state='processing' THEN 'processing' WHEN target.state='queued' OR source.state='queued' THEN 'queued' WHEN target.state='retry' OR source.state='retry' THEN 'retry' WHEN target.state='failed' OR source.state='failed' THEN 'failed' ELSE 'succeeded' END,attempts=MAX(target.attempts,source.attempts),next_attempt_at=CASE WHEN target.next_attempt_at IS NULL THEN source.next_attempt_at WHEN source.next_attempt_at IS NULL THEN target.next_attempt_at ELSE MIN(target.next_attempt_at,source.next_attempt_at) END,last_error=CASE WHEN target.last_error='' THEN source.last_error ELSE target.last_error END,welcome_pending=MAX(target.welcome_pending,source.welcome_pending),created_at=MIN(target.created_at,source.created_at),updated_at=MAX(target.updated_at,source.updated_at) FROM card_rename_jobs AS source WHERE target.account_id=? AND target.group_id=? AND target.user_id=? AND source.account_id=target.account_id AND source.group_id=target.group_id AND source.user_id=? AND source.desired_name=target.desired_name",
        params![account_id, group_id, real_user_id, old_user_id],
    )?;
    transaction.execute(
        "DELETE FROM card_rename_jobs WHERE account_id=? AND group_id=? AND user_id=?",
        params![account_id, group_id, old_user_id],
    )?;
    transaction.execute(
        "UPDATE member_identity_aliases SET user_id=?,provisional_user_id=CASE WHEN ?<0 THEN COALESCE(provisional_user_id,?) ELSE provisional_user_id END,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id,old_user_id,old_user_id,Utc::now().to_rfc3339(),account_id,group_id,old_user_id],
    )?;
    transaction.execute(
        "DELETE FROM members WHERE account_id=? AND group_id=? AND user_id=?",
        params![account_id, group_id, old_user_id],
    )?;
    Ok(())
}

/// Repairs pre-v3 records that stored a numeric NIM account as `user_id`
/// before the authoritative HTTP roster exposed the real 旺商号.  The alias
/// table is the proof of identity; an empty NIM binding plus internal hashes
/// prevent a real member whose numeric ID happens to match an account from
/// being merged by inference alone.
fn reconcile_alias_backed_legacy_members(connection: &mut Connection) -> rusqlite::Result<usize> {
    let transaction = connection.transaction()?;
    let candidates = {
        let mut statement = transaction.prepare(
            "SELECT alias.account_id,alias.group_id,legacy.user_id,alias.user_id,legacy.nickname,legacy.card_name
             FROM member_identity_aliases alias
             JOIN members legacy
               ON legacy.account_id=alias.account_id
              AND legacy.group_id=alias.group_id
              AND legacy.user_id=CAST(alias.nim_id AS INTEGER)
             WHERE alias.user_id>0
               AND CAST(alias.nim_id AS INTEGER)>0
               AND alias.user_id<>legacy.user_id
               AND legacy.nim_id=''",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut repaired = 0;
    for (account_id, group_id, legacy_user_id, canonical_user_id, nickname, card_name) in candidates
    {
        let legacy_looks_internal = [nickname.as_str(), card_name.as_str()]
            .iter()
            .all(|value| value.trim().is_empty() || is_internal_member_hash(value));
        if !legacy_looks_internal {
            continue;
        }
        merge_member_identity(
            &transaction,
            &account_id,
            group_id,
            legacy_user_id,
            canonical_user_id,
        )?;
        repaired += 1;
    }
    transaction.commit()?;
    Ok(repaired)
}

fn bool_i(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn parse_time(value: String) -> DateTime<Utc> {
    parse_stored_time(&value).unwrap_or_else(Utc::now)
}

fn optional_time(value: Option<String>) -> Option<DateTime<Utc>> {
    value.and_then(|value| parse_stored_time(&value))
}

fn parse_stored_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|time| time.and_utc())
        })
}

fn group_from_row(row: &Row<'_>) -> rusqlite::Result<Group> {
    Ok(Group {
        account_id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        owner_user_id: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        ai_enabled: row.get::<_, i64>(5)? != 0,
        moderation_enabled: row.get::<_, i64>(6)? != 0,
        machine_rules_enabled: row.get::<_, i64>(7)? != 0,
        ai_rules_enabled: row.get::<_, i64>(8)? != 0,
        manual_takeover: row.get::<_, i64>(9)? != 0,
        welcome_message: row.get(10)?,
        updated_at: parse_time(row.get(11)?),
    })
}

fn member_from_row(row: &Row<'_>) -> rusqlite::Result<Member> {
    Ok(Member {
        account_id: row.get(0)?,
        group_id: row.get(1)?,
        user_id: row.get(2)?,
        nim_id: row.get(3)?,
        nickname: row.get(4)?,
        card_name: row.get(5)?,
        original_card_name: row.get(6)?,
        managed_card_name: row.get(7)?,
        card_suffix: row.get(8)?,
        role: row.get(9)?,
        account_state: row.get(10)?,
        blacklisted: row.get::<_, i64>(11)? != 0,
        present: row.get::<_, i64>(12)? != 0,
        join_source: row.get(13)?,
        prompt_read: row.get::<_, i64>(14)? != 0,
        locked_card_name: row.get(15)?,
        violation_count: row.get(16)?,
        discovered_at: parse_time(row.get(17)?),
        joined_at: optional_time(row.get(18)?),
        last_seen_at: parse_time(row.get(19)?),
        updated_at: parse_time(row.get(20)?),
    })
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        account_id: row.get(1)?,
        group_id: row.get(2)?,
        server_message_id: row.get(3)?,
        sequence: row.get(4)?,
        user_id: row.get(5)?,
        sender_name: row.get(6)?,
        kind: row.get(7)?,
        text: row.get(8)?,
        sent_at: parse_time(row.get(9)?),
        received_at: parse_time(row.get(10)?),
        processed_at: optional_time(row.get(11)?),
        acknowledged_at: optional_time(row.get(12)?),
        processing_state: row.get(13)?,
        attempts: row.get(14)?,
        next_attempt_at: optional_time(row.get(15)?),
        last_error: row.get(16)?,
        mentions_json: row.get(17)?,
        source_kind: row.get(18)?,
        flow: row.get(19)?,
    })
}

fn gateway_inbox_from_row(row: &Row<'_>) -> rusqlite::Result<GatewayInboxItem> {
    Ok(GatewayInboxItem {
        id: row.get(0)?,
        account_id: row.get(1)?,
        bridge_session: row.get(2)?,
        bridge_sequence: row.get(3)?,
        event_id: row.get(4)?,
        event_type: row.get(5)?,
        payload_json: row.get(6)?,
        state: row.get(7)?,
        attempts: row.get(8)?,
        next_attempt_at: optional_time(row.get(9)?),
        last_error: row.get(10)?,
        received_at: parse_time(row.get(11)?),
        claimed_at: optional_time(row.get(12)?),
        processed_at: optional_time(row.get(13)?),
    })
}

fn effect_outbox_from_row(row: &Row<'_>) -> rusqlite::Result<EffectOutboxItem> {
    Ok(EffectOutboxItem {
        id: row.get(0)?,
        account_id: row.get(1)?,
        group_id: row.get(2)?,
        effect_type: row.get(3)?,
        payload_json: row.get(4)?,
        dedupe_key: row.get(5)?,
        state: row.get(6)?,
        attempts: row.get(7)?,
        next_attempt_at: optional_time(row.get(8)?),
        last_error: row.get(9)?,
        receipt_json: row.get(10)?,
        created_at: parse_time(row.get(11)?),
        claimed_at: optional_time(row.get(12)?),
        completed_at: optional_time(row.get(13)?),
        priority: row.get(14)?,
        lane: row.get(15)?,
        order_key: row.get(16)?,
        correlation_id: row.get(17)?,
        origin: row.get(18)?,
        expires_at: optional_time(row.get(19)?),
    })
}

fn card_job_from_row(row: &Row<'_>) -> rusqlite::Result<CardRenameJob> {
    Ok(CardRenameJob {
        id: row.get(0)?,
        account_id: row.get(1)?,
        group_id: row.get(2)?,
        user_id: row.get(3)?,
        nim_id: row.get(4)?,
        original_name: row.get(5)?,
        desired_name: row.get(6)?,
        suffix: row.get(7)?,
        state: row.get(8)?,
        attempts: row.get(9)?,
        next_attempt_at: row
            .get::<_, Option<String>>(10)?
            .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
            .map(|value| value.with_timezone(&Utc)),
        last_error: row.get(11)?,
        welcome_pending: row.get::<_, i64>(12)? != 0,
        created_at: parse_time(row.get(13)?),
        updated_at: parse_time(row.get(14)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RuleAction;
    use tempfile::tempdir;

    fn populated_database() -> Database {
        let directory = tempdir().unwrap().keep();
        let paths = AppPaths {
            root: directory.clone(),
            v3: directory.join("3.0"),
            database: directory.join("3.0/dh.db"),
            secrets: directory.join("3.0/secrets.dat"),
            logs: directory.join("3.0/logs"),
            legacy_backups: directory.join("legacy-backups"),
            runtime_mode_file: directory.join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "a".into(),
                display_name: "A".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: "a".into(),
                group_id: 1,
                name: "G".into(),
                owner_user_id: 1,
                enabled: true,
                ai_enabled: true,
                moderation_enabled: true,
                machine_rules_enabled: true,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        database
    }

    #[test]
    fn creates_clean_schema_and_defaults() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let status = database.status().unwrap();
        assert_eq!(status.schema_version, 14);
        assert_eq!(status.groups, 0);
        assert_eq!(
            database.get_setting("ai.model").unwrap().as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn expected_member_card_updates_are_exact_and_single_use() {
        let database = populated_database();
        database
            .expect_member_card_update("a", 1, 9, "DH群员0009", "test:rename")
            .unwrap();
        assert!(!database
            .consume_expected_member_card_update("a", 1, 9, "其他名称")
            .unwrap());
        assert!(database
            .consume_expected_member_card_update("a", 1, 9, "DH群员0009")
            .unwrap());
        assert!(!database
            .consume_expected_member_card_update("a", 1, 9, "DH群员0009")
            .unwrap());
    }

    #[test]
    fn migrates_legacy_ai_settings_to_one_account_scoped_endpoint() {
        let database = populated_database();
        database
            .set_setting("ai.base_url", "https://ai.example/v1", false)
            .unwrap();
        database
            .set_setting("ai.model", "deepseek-v4-pro", false)
            .unwrap();
        database
            .set_setting("ai.api_backend", "responses", false)
            .unwrap();
        database.ensure_ai_provider_endpoints("a").unwrap();
        database.ensure_ai_provider_endpoints("a").unwrap();
        let endpoints = database.list_ai_provider_endpoints("a").unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "主连接");
        assert_eq!(endpoints[0].base_url, "https://ai.example/v1");
        assert_eq!(endpoints[0].api_backend, "responses");
        assert_eq!(endpoints[0].secret_ref, "ai.api_key");
    }

    #[test]
    fn ai_endpoint_health_records_cooldown_and_recovers() {
        let database = populated_database();
        database.ensure_ai_provider_endpoints("a").unwrap();
        let endpoint_id = database.list_ai_provider_endpoints("a").unwrap()[0].id;
        database
            .update_ai_provider_health("a", endpoint_id, false, "AI 请求返回 HTTP 401")
            .unwrap();
        let failed = database.list_ai_provider_endpoints("a").unwrap().remove(0);
        assert_eq!(failed.health_status, "unavailable");
        assert_eq!(failed.failure_count, 1);
        assert!(failed.cooldown_until.is_some());
        database
            .update_ai_provider_health("a", endpoint_id, true, "")
            .unwrap();
        let recovered = database.list_ai_provider_endpoints("a").unwrap().remove(0);
        assert_eq!(recovered.health_status, "healthy");
        assert_eq!(recovered.failure_count, 0);
        assert!(recovered.cooldown_until.is_none());
    }

    #[test]
    fn ai_endpoint_transient_failures_back_off_exponentially() {
        let database = populated_database();
        database.ensure_ai_provider_endpoints("a").unwrap();
        let endpoint_id = database.list_ai_provider_endpoints("a").unwrap()[0].id;
        database
            .update_ai_provider_health("a", endpoint_id, false, "AI 请求返回 HTTP 503")
            .unwrap();
        let first = database.list_ai_provider_endpoints("a").unwrap().remove(0);
        database
            .update_ai_provider_health("a", endpoint_id, false, "AI 请求返回 HTTP 503")
            .unwrap();
        let second = database.list_ai_provider_endpoints("a").unwrap().remove(0);
        assert_eq!(second.failure_count, 2);
        assert!(second.cooldown_until.unwrap() > first.cooldown_until.unwrap());
    }

    #[test]
    fn capability_verification_is_scoped_to_protocol_fingerprint() {
        let database = populated_database();
        database
            .save_gateway_capability_verification(
                "fingerprint-a",
                "announcement",
                "manualReceipt",
                "supported",
                true,
                "evidence-a",
                "",
            )
            .unwrap();
        assert!(database
            .is_gateway_capability_verified("fingerprint-a", "announcement")
            .unwrap());
        assert!(!database
            .is_gateway_capability_verified("fingerprint-b", "announcement")
            .unwrap());
    }

    #[test]
    fn business_app_defaults_are_disabled_and_runs_are_idempotent() {
        let database = populated_database();
        database.ensure_account_defaults("a").unwrap();
        let apps = database.list_business_apps("a").unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app_id, "prediction");
        assert!(!apps[0].enabled);
        assert!(!database.business_app_enabled("a", "prediction").unwrap());
        let now = Utc::now();
        let run = BusinessAppRun {
            id: 0,
            account_id: "a".into(),
            app_id: "prediction".into(),
            group_id: 1,
            message_id: 9,
            run_key: "message:9".into(),
            status: "processing".into(),
            freshness: "missing".into(),
            ai_used: false,
            reply: String::new(),
            error: String::new(),
            elapsed_ms: 0,
            created_at: now,
            completed_at: None,
        };
        assert!(database.claim_business_app_run(&run).unwrap());
        assert!(!database.claim_business_app_run(&run).unwrap());
        database
            .finish_business_app_run(
                "a",
                "prediction",
                "message:9",
                "fallback",
                "fresh",
                false,
                "模板",
                "",
                12,
            )
            .unwrap();
        let runs = database
            .list_business_app_runs("a", "prediction", 10)
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "fallback");
        assert_eq!(runs[0].reply, "模板");
    }

    #[test]
    fn account_defaults_are_complete_disabled_and_idempotent() {
        let database = populated_database();
        database.ensure_account_defaults("a").unwrap();
        database.ensure_account_defaults("a").unwrap();
        database.with_connection(|connection| {
            let rule_count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM rules WHERE account_id='a'",
                [],
                |row| row.get(0),
            )?;
            let enabled_count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM rules WHERE account_id='a' AND enabled=1",
                [],
                |row| row.get(0),
            )?;
            let action_count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM rule_actions WHERE kind='recall'",
                [],
                |row| row.get(0),
            )?;
            let semantic_observe: i64 = connection.query_row(
                "SELECT COUNT(*) FROM rules WHERE account_id='a' AND matcher='semantic' AND mode='observe'",
                [],
                |row| row.get(0),
            )?;
            let (built_in, read_only, enabled): (i64, i64, i64) = connection.query_row(
                "SELECT built_in,read_only,enabled FROM knowledge_bases WHERE account_id='a' AND name=?",
                params![defaults::DEFAULT_KNOWLEDGE_BASE_NAME],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let document_count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM knowledge_documents WHERE base_id=(SELECT id FROM knowledge_bases WHERE account_id='a' AND name=?)",
                params![defaults::DEFAULT_KNOWLEDGE_BASE_NAME],
                |row| row.get(0),
            )?;
            let binding_count: i64 = connection.query_row(
                "SELECT COUNT(*) FROM knowledge_base_groups WHERE account_id='a'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(rule_count, defaults::default_rules("a").len() as i64);
            assert_eq!(enabled_count, 0);
            assert_eq!(action_count, defaults::default_rules("a").len() as i64);
            assert_eq!(semantic_observe, 3);
            assert_eq!((built_in, read_only, enabled), (1, 1, 1));
            assert_eq!(document_count, defaults::DEFAULT_KNOWLEDGE_DOCUMENTS.len() as i64);
            assert_eq!(binding_count, 0);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn account_defaults_preserve_same_name_custom_rule() {
        let database = populated_database();
        let mut custom_rule = defaults::default_rules("a").remove(0);
        custom_rule.matcher = "contains".into();
        custom_rule.pattern = "管理员自定义内容".into();
        custom_rule.enabled = true;
        custom_rule.actions = vec![RuleAction {
            kind: "reply".into(),
            duration_seconds: 0,
            message: "管理员自定义回复".into(),
        }];
        database.save_rule(&custom_rule).unwrap();

        database.ensure_account_defaults("a").unwrap();

        database
            .with_connection(|connection| {
                let (count, matcher, pattern, enabled): (i64, String, String, i64) = connection
                    .query_row(
                        "SELECT COUNT(*),matcher,pattern,enabled FROM rules WHERE account_id='a' AND group_id=0 AND name='加权字符超过 100'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )?;
                let actions: Vec<String> = connection
                    .prepare("SELECT a.kind FROM rule_actions a JOIN rules r ON r.id=a.rule_id WHERE r.account_id='a' AND r.name='加权字符超过 100' ORDER BY a.position")?
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<_, _>>()?;
                assert_eq!(count, 1);
                assert_eq!(matcher, "contains");
                assert_eq!(pattern, "管理员自定义内容");
                assert_eq!(enabled, 1);
                assert_eq!(actions, vec!["reply"]);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn account_defaults_are_isolated_and_builtin_clone_is_editable() {
        let database = populated_database();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "b".into(),
                display_name: "B".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database.ensure_account_defaults("a").unwrap();
        database.ensure_account_defaults("b").unwrap();

        let base_id = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT id FROM knowledge_bases WHERE account_id='a' AND name=?",
                    params![defaults::DEFAULT_KNOWLEDGE_BASE_NAME],
                    |row| row.get(0),
                )
            })
            .unwrap();
        let clone_id = database
            .clone_knowledge_base("a", base_id, "DH 默认群规与 FAQ（副本）")
            .unwrap();

        database
            .with_connection(|connection| {
                let account_a_rules: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM rules WHERE account_id='a'",
                    [],
                    |row| row.get(0),
                )?;
                let account_b_rules: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM rules WHERE account_id='b'",
                    [],
                    |row| row.get(0),
                )?;
                let (clone_account, built_in, read_only): (String, i64, i64) = connection
                    .query_row(
                        "SELECT account_id,built_in,read_only FROM knowledge_bases WHERE id=?",
                        params![clone_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                let clone_documents: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM knowledge_documents WHERE base_id=?",
                    params![clone_id],
                    |row| row.get(0),
                )?;
                assert_eq!(account_a_rules, defaults::default_rules("a").len() as i64);
                assert_eq!(account_b_rules, defaults::default_rules("b").len() as i64);
                assert_eq!((clone_account.as_str(), built_in, read_only), ("a", 0, 0));
                assert_eq!(
                    clone_documents,
                    defaults::DEFAULT_KNOWLEDGE_DOCUMENTS.len() as i64
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn rejects_corrupt_database_without_overwriting_it() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        std::fs::create_dir_all(&paths.v3).unwrap();
        let original = b"not-a-sqlite-database";
        std::fs::write(&paths.database, original).unwrap();
        let error = match Database::open(&paths) {
            Ok(_) => panic!("损坏数据库不应被当作新库覆盖"),
            Err(error) => error,
        };
        assert_eq!(error.code, "database_corrupt");
        assert_eq!(std::fs::read(&paths.database).unwrap(), original);
    }

    #[test]
    fn migrates_legacy_schema_and_keeps_snapshot() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        std::fs::create_dir_all(&paths.v3).unwrap();
        let legacy = Connection::open(&paths.database).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE members(account_id TEXT,group_id INTEGER,user_id INTEGER,nim_id TEXT,nickname TEXT,card_name TEXT,original_card_name TEXT,managed_card_name TEXT,card_suffix TEXT,role TEXT,account_state TEXT,blacklisted INTEGER,present INTEGER,updated_at TEXT,PRIMARY KEY(account_id,group_id,user_id));
                 CREATE TABLE tasks(id INTEGER PRIMARY KEY,account_id TEXT,group_id INTEGER,title TEXT,description TEXT,status TEXT,due_at TEXT,created_at TEXT,updated_at TEXT);
                 PRAGMA user_version=1;",
            )
            .unwrap();
        drop(legacy);

        let database = Database::open(&paths).unwrap();
        assert_eq!(database.status().unwrap().schema_version, 14);
        let columns = database
            .with_connection(|connection| {
                let mut statement = connection.prepare("PRAGMA table_info(members)")?;
                let columns = statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(columns)
            })
            .unwrap();
        assert!(columns.contains(&"join_source".into()));
        let task_columns = database
            .with_connection(|connection| {
                let mut statement = connection.prepare("PRAGMA table_info(tasks)")?;
                let result = statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .unwrap();
        assert!(task_columns.contains(&"reminder_state".into()));
        assert!(task_columns.contains(&"reminder_next_attempt_at".into()));
        assert_eq!(
            std::fs::read_dir(paths.v3.join("backups")).unwrap().count(),
            1
        );
        drop(database);
        let reopened = Database::open(&paths).unwrap();
        assert_eq!(reopened.status().unwrap().schema_version, 14);
        assert_eq!(
            std::fs::read_dir(paths.v3.join("backups")).unwrap().count(),
            1
        );
    }

    #[test]
    fn migrates_v4_receipts_transactionally_and_keeps_snapshot() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        std::fs::create_dir_all(&paths.v3).unwrap();
        let legacy = Connection::open(&paths.database).unwrap();
        legacy.execute_batch(SCHEMA).unwrap();
        legacy
            .execute_batch(
                "ALTER TABLE actions DROP COLUMN receipt_json;
                 ALTER TABLE effect_outbox DROP COLUMN receipt_json;
                 PRAGMA user_version=4;",
            )
            .unwrap();
        drop(legacy);

        let database = Database::open(&paths).unwrap();
        assert_eq!(database.status().unwrap().schema_version, 14);
        for table in ["actions", "effect_outbox"] {
            assert!(database
                .with_connection(|connection| table_has_column(connection, table, "receipt_json"))
                .unwrap());
        }
        let snapshots = std::fs::read_dir(paths.v3.join("backups"))
            .unwrap()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), 1);
    }

    #[test]
    fn migrates_v2_and_v3_to_v13_idempotently() {
        for version in [2_i64, 3_i64] {
            let directory = tempdir().unwrap();
            let paths = AppPaths {
                root: directory.path().to_path_buf(),
                v3: directory.path().join("3.0"),
                database: directory.path().join("3.0/dh.db"),
                secrets: directory.path().join("3.0/secrets.dat"),
                logs: directory.path().join("3.0/logs"),
                legacy_backups: directory.path().join("legacy-backups"),
                runtime_mode_file: directory.path().join("runtime-mode"),
            };
            std::fs::create_dir_all(&paths.v3).unwrap();
            let legacy = Connection::open(&paths.database).unwrap();
            legacy.execute_batch(SCHEMA).unwrap();
            legacy
                .execute_batch(&format!(
                    "ALTER TABLE actions DROP COLUMN receipt_json;
                     ALTER TABLE effect_outbox DROP COLUMN receipt_json;
                     PRAGMA user_version={version};"
                ))
                .unwrap();
            drop(legacy);

            let database = Database::open(&paths).unwrap();
            assert_eq!(database.status().unwrap().schema_version, 14);
            for table in ["actions", "effect_outbox"] {
                assert!(database
                    .with_connection(|connection| {
                        table_has_column(connection, table, "receipt_json")
                    })
                    .unwrap());
            }
            assert_eq!(
                std::fs::read_dir(paths.v3.join("backups")).unwrap().count(),
                1
            );
            drop(database);
            let reopened = Database::open(&paths).unwrap();
            assert_eq!(reopened.status().unwrap().schema_version, 14);            assert_eq!(
                std::fs::read_dir(paths.v3.join("backups")).unwrap().count(),
                1
            );
        }
    }

    #[test]
    fn outstanding_task_reminders_keep_firing_after_activity_migration() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        std::fs::create_dir_all(&paths.v3).unwrap();

        // 造一个 v11 库：一条还没发出的未来提醒，一条已经发过的提醒。
        let pending_at = "2031-09-15T10:00:00+00:00";
        let legacy = Connection::open(&paths.database).unwrap();
        legacy.execute_batch(SCHEMA).unwrap();
        legacy
            .execute_batch(&format!(
                "INSERT INTO accounts(id,display_name,role,discovered_at,updated_at) VALUES('acct','','unknown','2026-01-01T00:00:00+00:00','2026-01-01T00:00:00+00:00');
                 INSERT INTO groups(account_id,group_id,name,updated_at) VALUES('acct',77,'测试群','2026-01-01T00:00:00+00:00');
                 INSERT INTO tasks(id,account_id,group_id,title,description,status,reminder_at,reminder_state,created_at,updated_at)
                   VALUES(1,'acct',77,'待发提醒','记得开播','pending','{pending_at}','pending','2026-01-01T00:00:00+00:00','2026-01-01T00:00:00+00:00');
                 INSERT INTO tasks(id,account_id,group_id,title,description,status,reminder_at,reminder_sent_at,reminder_state,created_at,updated_at)
                   VALUES(2,'acct',77,'已发提醒','历史','done','2026-02-01T10:00:00+00:00','2026-02-01T10:00:05+00:00','sent','2026-01-01T00:00:00+00:00','2026-01-01T00:00:00+00:00');
                 PRAGMA user_version=11;"
            ))
            .unwrap();
        drop(legacy);

        let database = Database::open(&paths).unwrap();
        assert_eq!(database.status().unwrap().schema_version, 14);
        let (enabled, next_run_at, local_time): (i64, Option<String>, String) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT activities.enabled,activities.next_run_at,activity_times.local_time \
                     FROM activities JOIN activity_times ON activity_times.activity_id=activities.id \
                     WHERE activities.source_key='legacy-task:1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(enabled, 1);
        assert_eq!(next_run_at.as_deref(), Some(pending_at));

        // local_time 是 timezone 列所指时区的墙上时间，不能是原始 UTC 的 10:00。
        let expected_local = DateTime::parse_from_rfc3339(pending_at)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string();
        assert_eq!(local_time, expected_local);

        // 已经发出的提醒保持停用草稿，不重复轰炸群。
        let sent_enabled: i64 = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT enabled FROM activities WHERE source_key='legacy-task:2'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(sent_enabled, 0);

        // 版本闸：迁移每次启动都会跑，但不能反复覆盖用户后来手工停用的活动。
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE activities SET enabled=0 WHERE source_key='legacy-task:1'",
                    [],
                )
            })
            .unwrap();
        drop(database);
        let reopened = Database::open(&paths).unwrap();
        let still_disabled: i64 = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT enabled FROM activities WHERE source_key='legacy-task:1'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(still_disabled, 0);
    }

    #[test]
    fn duplicate_messages_are_idempotent() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "a".into(),
                display_name: "A".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: "a".into(),
                group_id: 1,
                name: "G".into(),
                owner_user_id: 1,
                enabled: true,
                ai_enabled: true,
                moderation_enabled: true,
                machine_rules_enabled: true,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        let message = Message {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            server_message_id: "s1".into(),
            sequence: 1,
            user_id: 2,
            sender_name: "M".into(),
            kind: "text".into(),
            text: "hello".into(),
            sent_at: now,
            received_at: now,
            processed_at: None,
            acknowledged_at: None,
            processing_state: "pending".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: None,
            flow: None,
        };
        let first = database.insert_message(&message).unwrap();
        assert!(first.inserted);
        assert!(first.id > 0);
        let second = database.insert_message(&message).unwrap();
        assert!(!second.inserted);
        assert_eq!(second.id, first.id);
    }

    #[test]
    fn gateway_inbox_and_effect_outbox_are_atomic_and_deduplicated() {
        let database = populated_database();
        let now = Utc::now();
        let events = vec![
            GatewayInboxEvent {
                account_id: "a".into(),
                bridge_session: "session-a".into(),
                bridge_sequence: 1,
                event_id: "event-1".into(),
                event_type: "message".into(),
                payload_json: "{\"sequence\":1}".into(),
                received_at: now,
            },
            GatewayInboxEvent {
                account_id: "a".into(),
                bridge_session: "session-a".into(),
                bridge_sequence: 2,
                event_id: "event-2".into(),
                event_type: "member".into(),
                payload_json: "{}".into(),
                received_at: now,
            },
        ];
        let first = database.ingest_gateway_inbox_batch(&events).unwrap();
        assert_eq!((first.inserted, first.duplicates), (2, 0));
        let duplicate = database.ingest_gateway_inbox_batch(&events).unwrap();
        assert_eq!((duplicate.inserted, duplicate.duplicates), (0, 2));
        assert_eq!(
            database
                .mark_gateway_inbox_processed("a", &["event-2".into(), "missing".into()])
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .processed_gateway_event_ids("a", &["event-1".into(), "event-2".into()])
                .unwrap(),
            vec!["event-2".to_string()]
        );
        let claimed = database.claim_gateway_inbox(Some("a"), 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(database
            .claim_gateway_inbox(Some("a"), 10)
            .unwrap()
            .is_empty());
        database
            .finish_gateway_inbox(claimed[0].id, true, "")
            .unwrap();

        let effect = EffectOutboxRequest {
            account_id: "a".into(),
            group_id: 1,
            effect_type: "send_message".into(),
            payload_json: "{\"text\":\"hello\"}".into(),
            dedupe_key: "reply:event-1".into(),
        };
        let enqueued = database.enqueue_effect(&effect).unwrap();
        assert!(enqueued.inserted);
        let duplicate = database.enqueue_effect(&effect).unwrap();
        assert_eq!(duplicate.id, enqueued.id);
        assert!(!duplicate.inserted);
        let claimed_effects = database.claim_effect_outbox(Some("a"), 10).unwrap();
        assert_eq!(claimed_effects.len(), 1);
        database.prepare().unwrap();
        assert!(database
            .claim_effect_outbox(Some("a"), 10)
            .unwrap()
            .is_empty());
        assert_eq!(database.enqueue_effect(&effect).unwrap().state, "unknown");
        let unknown_receipt: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT receipt_json FROM effect_outbox WHERE id=?",
                    params![claimed_effects[0].id],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(unknown_receipt.contains(r#""status":"unknown""#));
        assert!(database.retry_effect_outbox(claimed_effects[0].id).unwrap());
        let claimed_effects = database.claim_effect_outbox(Some("a"), 10).unwrap();
        assert_eq!(claimed_effects.len(), 1);
        database
            .finish_effect_outbox(
                claimed_effects[0].id,
                true,
                "",
                r#"{"requestId":"fixture-request-1","status":"succeeded"}"#,
            )
            .unwrap();
        let archived_receipt: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT receipt_json FROM effect_outbox WHERE id=?",
                    params![claimed_effects[0].id],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(archived_receipt.contains("fixture-request-1"));
        assert!(database
            .claim_effect_outbox(Some("a"), 10)
            .unwrap()
            .is_empty());
        assert_eq!(database.enqueue_effect(&effect).unwrap().state, "succeeded");
    }

    #[test]
    fn restart_recovers_dispatched_effect_as_unknown_with_action_and_audit() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "a".into(),
                display_name: "A".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: "a".into(),
                group_id: 1,
                name: "G".into(),
                owner_user_id: 1,
                enabled: true,
                ai_enabled: true,
                moderation_enabled: true,
                machine_rules_enabled: true,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        let effect = database
            .enqueue_effect(&EffectOutboxRequest {
                account_id: "a".into(),
                group_id: 1,
                effect_type: "send_text".into(),
                payload_json: serde_json::json!({
                    "text": "hello",
                    "recordAction": true,
                    "actionKind": "reply",
                    "userId": 2,
                    "messageId": 3,
                    "ruleId": 4,
                    "mode": "automatic",
                    "reason": "restart-cutpoint",
                })
                .to_string(),
                dedupe_key: "restart-cutpoint-effect".into(),
            })
            .unwrap();
        assert_eq!(database.claim_effect_outbox(Some("a"), 1).unwrap().len(), 1);
        drop(database);

        let recovered = Database::open(&paths).unwrap();
        let (state, outbox_receipt, action_success, action_receipt, audit_details): (
            String,
            String,
            i64,
            String,
            String,
        ) = recovered
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT state FROM effect_outbox WHERE id=?",
                        params![effect.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT receipt_json FROM effect_outbox WHERE id=?",
                        params![effect.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT success FROM actions WHERE dedupe_key='restart-cutpoint-effect'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT receipt_json FROM actions WHERE dedupe_key='restart-cutpoint-effect'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT details FROM audit_events WHERE event='effect_recovered_unknown'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(state, "unknown");
        assert_eq!(action_success, 0);
        for archived in [outbox_receipt, action_receipt, audit_details] {
            assert!(archived.contains("unknown"));
            assert!(archived.contains("远端执行结果未知"));
        }
    }

    #[test]
    fn gateway_inbox_retry_never_skips_an_unconfirmed_lower_sequence() {
        let database = populated_database();
        let now = Utc::now();
        let events = [1_i64, 2_i64]
            .into_iter()
            .map(|sequence| GatewayInboxEvent {
                account_id: "a".into(),
                bridge_session: "ordered-session".into(),
                bridge_sequence: sequence,
                event_id: format!("ordered-{sequence}"),
                event_type: "message".into(),
                payload_json: format!("{{\"sequence\":{sequence}}}"),
                received_at: now,
            })
            .collect::<Vec<_>>();
        database.ingest_gateway_inbox_batch(&events).unwrap();
        let first = database.claim_gateway_inbox(Some("a"), 10).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].bridge_sequence, 1);
        database
            .finish_gateway_inbox(first[0].id, false, "ACK 失败")
            .unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE gateway_inbox SET state='failed',next_attempt_at=NULL WHERE id=?",
                    params![first[0].id],
                )
            })
            .unwrap();
        assert!(database
            .claim_gateway_inbox(Some("a"), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn ignored_gateway_inbox_event_keeps_its_diagnostic_without_blocking_following_sequence() {
        let database = populated_database();
        let now = Utc::now();
        let events = [1_i64, 2_i64]
            .into_iter()
            .map(|sequence| GatewayInboxEvent {
                account_id: "a".into(),
                bridge_session: "ignored-session".into(),
                bridge_sequence: sequence,
                event_id: format!("ignored-{sequence}"),
                event_type: "message".into(),
                payload_json: format!("{{\"sequence\":{sequence}}}"),
                received_at: now,
            })
            .collect::<Vec<_>>();
        database.ingest_gateway_inbox_batch(&events).unwrap();
        database
            .ignore_gateway_inbox_event("a", "ignored-1", "解码失败且群身份未映射")
            .unwrap();

        let claimed = database.claim_gateway_inbox(Some("a"), 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].bridge_sequence, 2);
        database
            .commit_gateway_ack("a", &["ignored-1".into()], &[])
            .unwrap();

        let (state, last_error): (String, String) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT state,last_error FROM gateway_inbox WHERE account_id='a' AND event_id='ignored-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(state, "ignored");
        assert_eq!(last_error, "解码失败且群身份未映射");
        assert_eq!(
            database
                .processed_gateway_event_ids("a", &["ignored-1".into()])
                .unwrap(),
            vec!["ignored-1".to_string()]
        );
    }

    #[test]
    fn gateway_ack_commit_is_atomic_and_retryable_after_local_failure() {
        let database = populated_database();
        let now = Utc::now();
        database
            .ingest_gateway_inbox_batch(&[GatewayInboxEvent {
                account_id: "a".into(),
                bridge_session: "session-ack".into(),
                bridge_sequence: 7,
                event_id: "event-ack".into(),
                event_type: "message".into(),
                payload_json: "{\"idServer\":\"message-ack\"}".into(),
                received_at: now,
            }])
            .unwrap();
        let persisted = database
            .insert_message(&Message {
                id: 0,
                account_id: "a".into(),
                group_id: 1,
                server_message_id: "message-ack".into(),
                sequence: 7,
                user_id: 2,
                sender_name: "M".into(),
                kind: "text".into(),
                text: "hello".into(),
                sent_at: now,
                received_at: now,
                processed_at: None,
                acknowledged_at: None,
                processing_state: "pending".into(),
                attempts: 0,
                next_attempt_at: None,
                last_error: String::new(),
                mentions_json: "[]".into(),
                source_kind: Some("nim".into()),
                flow: Some("inbound".into()),
            })
            .unwrap();

        database
            .with_connection(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_gateway_ack
                     BEFORE UPDATE OF state ON gateway_inbox
                     WHEN NEW.event_id='event-ack'
                     BEGIN SELECT RAISE(ABORT, 'injected local commit failure'); END;",
                )
            })
            .unwrap();
        let error = database
            .commit_gateway_ack("a", &["event-ack".into()], &[persisted.id])
            .unwrap_err();
        assert_eq!(error.code, "gateway_ack_commit");
        let (acknowledged_at, inbox_state): (Option<String>, String) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT acknowledged_at FROM messages WHERE id=?",
                        params![persisted.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT state FROM gateway_inbox WHERE account_id='a' AND event_id='event-ack'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert!(acknowledged_at.is_none());
        assert_eq!(inbox_state, "pending");

        database
            .with_connection(|connection| connection.execute_batch("DROP TRIGGER fail_gateway_ack"))
            .unwrap();
        database
            .commit_gateway_ack("a", &["event-ack".into()], &[persisted.id])
            .unwrap();
        database
            .commit_gateway_ack("a", &["event-ack".into()], &[persisted.id])
            .unwrap();
        let (acknowledged, processed): (i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT acknowledged_at IS NOT NULL FROM messages WHERE id=?",
                        params![persisted.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT state='processed' FROM gateway_inbox WHERE account_id='a' AND event_id='event-ack'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!((acknowledged, processed), (1, 1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn database_executor_serializes_ingest_and_claims_off_async_workers() {
        let database = populated_database();
        let executor = DatabaseExecutor::start(database).unwrap();
        let worker_name = executor
            .execute(|_| {
                Ok(std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string())
            })
            .await
            .unwrap();
        assert_eq!(worker_name, "dh-sqlite");

        let now = Utc::now();
        let event = GatewayInboxEvent {
            account_id: "a".into(),
            bridge_session: "executor-session".into(),
            bridge_sequence: 1,
            event_id: "executor-event".into(),
            event_type: "message".into(),
            payload_json: "{}".into(),
            received_at: now,
        };
        let message = Message {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            server_message_id: "executor-message".into(),
            sequence: 1,
            user_id: 2,
            sender_name: "M".into(),
            kind: "text".into(),
            text: "hello".into(),
            sent_at: now,
            received_at: now,
            processed_at: None,
            acknowledged_at: None,
            processing_state: "pending".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: Some("fixture".into()),
            flow: Some("inbound".into()),
        };
        let (batch, persisted) = executor
            .ingest_gateway_batch(vec![event], vec![message])
            .await
            .unwrap();
        assert_eq!(batch.inserted, 1);
        assert_eq!(persisted.len(), 1);
        assert!(persisted[0].inserted);

        let first_executor = executor.clone();
        let second_executor = executor.clone();
        let (first, second) = tokio::join!(
            first_executor.claim_gateway_inbox(Some("a".into()), 10),
            second_executor.claim_gateway_inbox(Some("a".into()), 10)
        );
        let mut claimed = first.unwrap();
        claimed.extend(second.unwrap());
        assert_eq!(claimed.len(), 1);
        executor
            .finish_gateway_inbox(claimed[0].id, true, String::new())
            .await
            .unwrap();

        let effect = executor
            .enqueue_effect(EffectOutboxRequest {
                account_id: "a".into(),
                group_id: 1,
                effect_type: "send_text".into(),
                payload_json: "{\"text\":\"hello\"}".into(),
                dedupe_key: "executor-effect".into(),
            })
            .await
            .unwrap();
        assert!(effect.inserted);
        let first_executor = executor.clone();
        let second_executor = executor.clone();
        let (first, second) = tokio::join!(
            first_executor.claim_effect_outbox(Some("a".into()), 10),
            second_executor.claim_effect_outbox(Some("a".into()), 10)
        );
        let mut claimed_effects = first.unwrap();
        claimed_effects.extend(second.unwrap());
        assert_eq!(claimed_effects.len(), 1);
        executor
            .finish_effect_outbox(claimed_effects[0].id, true, String::new(), "{}".into())
            .await
            .unwrap();

        executor
            .mark_message_acknowledged(persisted[0].id)
            .await
            .unwrap();
        assert!(executor.claim_message(persisted[0].id).await.unwrap());
        executor
            .finish_message_processing(persisted[0].id, true, String::new())
            .await
            .unwrap();
        assert!(executor
            .list_pending_messages("a".into(), 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(executor.status().await.unwrap().messages, 1);
        executor.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn database_executor_does_not_block_the_async_runtime() {
        let executor = DatabaseExecutor::start(populated_database()).unwrap();
        let slow_executor = executor.clone();
        let slow = tokio::spawn(async move {
            slow_executor
                .execute(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_millis(40),
            tokio::time::sleep(std::time::Duration::from_millis(10)),
        )
        .await
        .expect("SQLite 任务不应阻塞当前 Tokio 线程");
        slow.await.unwrap().unwrap();
        executor.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn database_executor_shutdown_drains_queued_work_and_rejects_new_jobs() {
        let executor = DatabaseExecutor::start(populated_database()).unwrap();
        let slow_executor = executor.clone();
        let slow = tokio::spawn(async move {
            slow_executor
                .execute(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    Ok(1_i64)
                })
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let write_executor = executor.clone();
        let queued = tokio::spawn(async move {
            write_executor
                .set_setting("shutdown.drained".into(), "true".into(), false)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        executor.shutdown().await.unwrap();
        assert_eq!(slow.await.unwrap().unwrap(), 1);
        queued.await.unwrap().unwrap();
        let error = executor.status().await.unwrap_err();
        assert_eq!(error.code, "database_executor_stopped");
        executor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_archives_inflight_effects_as_unknown() {
        let executor = DatabaseExecutor::start(populated_database()).unwrap();
        executor
            .enqueue_effect(EffectOutboxRequest {
                account_id: "a".into(),
                group_id: 1,
                effect_type: "send_text".into(),
                payload_json: serde_json::json!({
                    "text": "hello",
                    "recordAction": true,
                    "actionKind": "reply",
                    "userId": 2,
                    "reason": "shutdown-cutpoint",
                })
                .to_string(),
                dedupe_key: "shutdown-unknown".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            executor
                .claim_effect_outbox(Some("a".into()), 1)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            executor
                .mark_processing_effects_unknown("退出等待超时".into())
                .await
                .unwrap(),
            1
        );
        let (state, receipt, action_receipt, audit_details): (String, String, String, String) = executor
            .execute(|database| {
                database
                    .with_connection(|connection| {
                        Ok((
                            connection.query_row(
                                "SELECT state FROM effect_outbox WHERE dedupe_key='shutdown-unknown'",
                                [],
                                |row| row.get(0),
                            )?,
                            connection.query_row(
                                "SELECT receipt_json FROM effect_outbox WHERE dedupe_key='shutdown-unknown'",
                                [],
                                |row| row.get(0),
                            )?,
                            connection.query_row(
                                "SELECT receipt_json FROM actions WHERE dedupe_key='shutdown-unknown'",
                                [],
                                |row| row.get(0),
                            )?,
                            connection.query_row(
                                "SELECT details FROM audit_events WHERE event='effect_recovered_unknown'",
                                [],
                                |row| row.get(0),
                            )?,
                        ))
                    })
                    .map_err(|error| AppError::new("test_query", error.to_string()))
            })
            .await
            .unwrap();
        assert_eq!(state, "unknown");
        for archived in [receipt, action_receipt, audit_details] {
            assert!(archived.contains("退出等待超时"));
            assert!(archived.contains("unknown"));
        }
        executor.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn ambiguous_effect_receipt_is_terminal_until_manual_retry() {
        let executor = DatabaseExecutor::start(populated_database()).unwrap();
        executor
            .enqueue_effect(EffectOutboxRequest {
                account_id: "a".into(),
                group_id: 1,
                effect_type: "send_text".into(),
                payload_json: "{\"text\":\"hello\"}".into(),
                dedupe_key: "ambiguous-effect".into(),
            })
            .await
            .unwrap();
        let claimed = executor
            .claim_effect_outbox(Some("a".into()), 1)
            .await
            .unwrap();
        executor
            .finish_effect_outbox_unknown(
                claimed[0].id,
                "远端结果未确认".into(),
                "{\"status\":\"unknown\"}".into(),
            )
            .await
            .unwrap();
        assert!(executor
            .claim_effect_outbox(Some("a".into()), 10)
            .await
            .unwrap()
            .is_empty());
        let state = executor
            .execute(|database| {
                database
                    .with_connection(|connection| {
                        connection.query_row(
                            "SELECT state FROM effect_outbox WHERE dedupe_key='ambiguous-effect'",
                            [],
                            |row| row.get::<_, String>(0),
                        )
                    })
                    .map_err(|error| AppError::new("test_query", error.to_string()))
            })
            .await
            .unwrap();
        assert_eq!(state, "unknown");
        executor.shutdown().await.unwrap();
    }

    #[test]
    fn real_member_identity_merges_provisional_state_transactionally() {
        let database = populated_database();
        let now = Utc::now();
        let provisional = Member {
            account_id: "a".into(),
            group_id: 1,
            user_id: -22,
            nim_id: "nim-22".into(),
            nickname: "provisional".into(),
            card_name: "old card".into(),
            original_card_name: "old card".into(),
            managed_card_name: "DH0001".into(),
            card_suffix: "0001".into(),
            role: "member".into(),
            account_state: String::new(),
            blacklisted: true,
            present: true,
            join_source: "message-discovered".into(),
            prompt_read: false,
            locked_card_name: "DH0001".into(),
            violation_count: 3,
            discovered_at: now,
            joined_at: Some(now),
            last_seen_at: now,
            updated_at: now,
        };
        database.upsert_member(&provisional).unwrap();
        let mut real = provisional.clone();
        real.user_id = 22;
        real.nickname = "real".into();
        real.card_name = "wire card".into();
        real.original_card_name.clear();
        real.managed_card_name.clear();
        real.card_suffix.clear();
        real.blacklisted = false;
        real.prompt_read = true;
        real.locked_card_name.clear();
        real.violation_count = 0;
        database.upsert_member_from_wire(&real).unwrap();

        let members = database.list_members("a", 1).unwrap();
        assert_eq!(members.len(), 1);
        let saved = &members[0];
        assert_eq!(saved.user_id, 22);
        assert_eq!(saved.nickname, "real");
        assert_eq!(saved.card_name, "wire card");
        assert_eq!(saved.managed_card_name, "DH0001");
        assert_eq!(saved.card_suffix, "0001");
        assert!(saved.blacklisted);
        assert!(!saved.prompt_read);
        assert_eq!(saved.violation_count, 3);
        assert_eq!(
            database.resolve_member_user_id("a", 1, "nim-22").unwrap(),
            Some(22)
        );
    }

    #[test]
    fn authoritative_roster_merges_legacy_positive_nim_identity_and_references() {
        let database = populated_database();
        let now = Utc::now();
        let canonical = Member {
            account_id: "a".into(),
            group_id: 1,
            user_id: 22,
            nim_id: String::new(),
            nickname: "真实成员".into(),
            card_name: "真实名片".into(),
            original_card_name: String::new(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: String::new(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: now,
            joined_at: None,
            last_seen_at: now,
            updated_at: now,
        };
        database.upsert_member(&canonical).unwrap();
        let stamp = now.to_rfc3339();
        database
            .with_connection(|connection| {
                connection.execute_batch(&format!(
                    "INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,role,present,join_source,discovered_at,last_seen_at,updated_at) VALUES('a',1,9002,'','270dbd1d10bfb72ffa0ce988028c8133','270dbd1d10bfb72ffa0ce988028c8133','member',1,'message-discovered','{stamp}','{stamp}','{stamp}');
                     INSERT INTO messages(account_id,group_id,server_message_id,user_id,sent_at,received_at) VALUES('a',1,'legacy-message',9002,'{stamp}','{stamp}');
                     INSERT INTO actions(account_id,group_id,user_id,kind,created_at) VALUES('a',1,9002,'recall','{stamp}');
                     INSERT INTO audit_events(account_id,group_id,user_id,actor,event,created_at) VALUES('a',1,9002,'system','legacy','{stamp}');
                     INSERT INTO rules(account_id,name,matcher,created_at,updated_at) VALUES('a','legacy-rule','contains','{stamp}','{stamp}');
                     INSERT INTO rule_evaluations(account_id,group_id,user_id,message_id,rule_id,rule_type,mode,decision,created_at) VALUES('a',1,9002,1,last_insert_rowid(),'machine','automatic','matched','{stamp}');
                     INSERT INTO rule_runtime_state(rule_id,account_id,group_id,user_id,window_count,updated_at) SELECT id,'a',1,9002,4,'{stamp}' FROM rules WHERE name='legacy-rule';
                     INSERT INTO rule_whitelist_members(rule_id,user_id) SELECT id,9002 FROM rules WHERE name='legacy-rule';
                     INSERT INTO effect_outbox(account_id,group_id,effect_type,payload_json,dedupe_key,state,created_at) VALUES('a',1,'recall','{{\"userId\":9002}}','legacy-queued','queued','{stamp}');
                     INSERT INTO effect_outbox(account_id,group_id,effect_type,payload_json,dedupe_key,state,created_at) VALUES('a',1,'recall','{{\"userId\":9002}}','legacy-succeeded','succeeded','{stamp}');
                     INSERT INTO card_rename_jobs(account_id,group_id,user_id,desired_name,state,attempts,welcome_pending,created_at,updated_at) VALUES('a',1,22,'DH群员0001','succeeded',1,0,'{stamp}','{stamp}');
                     INSERT INTO card_rename_jobs(account_id,group_id,user_id,desired_name,state,attempts,welcome_pending,created_at,updated_at) VALUES('a',1,9002,'DH群员0001','retry',4,1,'{stamp}','{stamp}');"
                ))
            })
            .unwrap();

        let mut wire = canonical.clone();
        wire.nim_id = "9002".into();
        database.upsert_member_from_wire(&wire).unwrap();
        database.upsert_member_from_wire(&wire).unwrap();

        let members = database.list_members("a", 1).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, 22);
        assert_eq!(members[0].nim_id, "9002");
        let result: (i64, i64, i64, i64, i64, i64, i64, String, String, i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row("SELECT COUNT(*) FROM messages WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM actions WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM audit_events WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM rule_evaluations WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM rule_runtime_state WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM rule_whitelist_members WHERE user_id=22", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM members WHERE user_id=9002", [], |row| row.get(0))?,
                    connection.query_row("SELECT payload_json FROM effect_outbox WHERE dedupe_key='legacy-queued'", [], |row| row.get(0))?,
                    connection.query_row("SELECT payload_json FROM effect_outbox WHERE dedupe_key='legacy-succeeded'", [], |row| row.get(0))?,
                    connection.query_row("SELECT attempts FROM card_rename_jobs WHERE user_id=22 AND desired_name='DH群员0001'", [], |row| row.get(0))?,
                    connection.query_row("SELECT welcome_pending FROM card_rename_jobs WHERE user_id=22 AND desired_name='DH群员0001'", [], |row| row.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(
            [result.0, result.1, result.2, result.3, result.4, result.5],
            [1; 6]
        );
        assert_eq!(result.6, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&result.7).unwrap()["userId"],
            22
        );
        assert_eq!(
            serde_json::from_str::<Value>(&result.8).unwrap()["userId"],
            9002
        );
        assert_eq!(result.9, 4);
        assert_eq!(result.10, 1);
    }

    #[test]
    fn startup_reconciles_alias_backed_legacy_positive_nim_identity() {
        let database = populated_database();
        let now = Utc::now();
        let stamp = now.to_rfc3339();
        database
            .with_connection(|connection| {
                connection.execute_batch(&format!(
                    "INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,role,present,join_source,discovered_at,last_seen_at,updated_at) VALUES
                       ('a',1,22,'9002','真实成员','真实名片','member',1,'baseline','{stamp}','{stamp}','{stamp}'),
                       ('a',1,9002,'','270dbd1d10bfb72ffa0ce988028c8133','270dbd1d10bfb72ffa0ce988028c8133','member',1,'message-discovered','{stamp}','{stamp}','{stamp}');
                     INSERT INTO member_identity_aliases(account_id,group_id,nim_id,user_id,created_at,updated_at) VALUES('a',1,'9002',22,'{stamp}','{stamp}');
                     INSERT INTO messages(account_id,group_id,server_message_id,user_id,sent_at,received_at) VALUES('a',1,'startup-legacy-message',9002,'{stamp}','{stamp}');
                     INSERT INTO effect_outbox(account_id,group_id,effect_type,payload_json,dedupe_key,state,created_at) VALUES('a',1,'recall','{{\"userId\":9002}}','startup-legacy-effect','queued','{stamp}');"
                ))?;
                assert_eq!(reconcile_alias_backed_legacy_members(connection)?, 1);
                assert_eq!(reconcile_alias_backed_legacy_members(connection)?, 0);
                Ok(())
            })
            .unwrap();

        let result: (i64, i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT COUNT(*) FROM members WHERE account_id='a' AND group_id=1 AND user_id=9002",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT user_id FROM messages WHERE server_message_id='startup-legacy-message'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT CAST(json_extract(payload_json,'$.userId') AS INTEGER) FROM effect_outbox WHERE dedupe_key='startup-legacy-effect'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(result, (0, 22, 22));
    }

    #[test]
    fn startup_archives_only_stale_unacknowledged_messages() {
        let database = populated_database();
        let now = Utc::now();
        let stale = now - ChronoDuration::minutes(16);
        database
            .with_connection(|connection| {
                for (server_message_id, received_at, acknowledged_at) in [
                    ("stale-unacknowledged", stale, None),
                    ("recent-unacknowledged", now, None),
                    ("stale-acknowledged", stale, Some(stale)),
                ] {
                    connection.execute(
                        "INSERT INTO messages(account_id,group_id,server_message_id,user_id,sent_at,received_at,acknowledged_at,processing_state) VALUES('a',1,?,2,?,?,?,'pending')",
                        params![server_message_id,received_at.to_rfc3339(),received_at.to_rfc3339(),acknowledged_at.map(|value| value.to_rfc3339())],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        database.prepare().unwrap();
        let states = database
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT server_message_id,processing_state,processed_at IS NOT NULL FROM messages WHERE server_message_id LIKE '%unacknowledged' OR server_message_id='stale-acknowledged' ORDER BY server_message_id",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        assert_eq!(
            states,
            vec![
                ("recent-unacknowledged".into(), "pending".into(), 0),
                ("stale-acknowledged".into(), "pending".into(), 0),
                ("stale-unacknowledged".into(), "ignored".into(), 1),
            ]
        );
    }

    #[test]
    fn message_ack_and_processing_are_separate_and_recoverable() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "a".into(),
                display_name: "A".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: "a".into(),
                group_id: 1,
                name: "G".into(),
                owner_user_id: 1,
                enabled: true,
                ai_enabled: false,
                moderation_enabled: false,
                machine_rules_enabled: false,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        let message = Message {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            server_message_id: "recover".into(),
            sequence: 1,
            user_id: 2,
            sender_name: "M".into(),
            kind: "text".into(),
            text: "hello".into(),
            sent_at: now,
            received_at: now,
            processed_at: None,
            acknowledged_at: None,
            processing_state: "pending".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: None,
            flow: None,
        };
        let persisted = database.insert_message(&message).unwrap();
        assert!(database.list_pending_messages("a", 10).unwrap().is_empty());
        database.mark_message_acknowledged(persisted.id).unwrap();
        assert!(database.claim_message(persisted.id).unwrap());
        assert!(!database.claim_message(persisted.id).unwrap());
        database
            .finish_message_processing(persisted.id, false, "temporary")
            .unwrap();
        assert_eq!(database.list_pending_messages("a", 10).unwrap().len(), 0);
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE messages SET next_attempt_at=? WHERE id=?",
                    params![Utc::now().to_rfc3339(), persisted.id],
                )
            })
            .unwrap();
        assert_eq!(database.list_pending_messages("a", 10).unwrap().len(), 1);
        database
            .finish_message_processing(persisted.id, true, "")
            .unwrap();
        assert!(database.list_pending_messages("a", 10).unwrap().is_empty());
    }

    #[test]
    fn card_jobs_are_claimed_and_commit_member_state() {
        let directory = tempdir().unwrap();
        let paths = AppPaths {
            root: directory.path().to_path_buf(),
            v3: directory.path().join("3.0"),
            database: directory.path().join("3.0/dh.db"),
            secrets: directory.path().join("3.0/secrets.dat"),
            logs: directory.path().join("3.0/logs"),
            legacy_backups: directory.path().join("legacy-backups"),
            runtime_mode_file: directory.path().join("runtime-mode"),
        };
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: "a".into(),
                display_name: "A".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: "a".into(),
                group_id: 1,
                name: "G".into(),
                owner_user_id: 1,
                enabled: true,
                ai_enabled: false,
                moderation_enabled: false,
                machine_rules_enabled: false,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        let member = Member {
            account_id: "a".into(),
            group_id: 1,
            user_id: 2,
            nim_id: "nim-2".into(),
            nickname: "原名".into(),
            card_name: "原名".into(),
            original_card_name: "原名".into(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: "ACCOUNT_STATE_GOOD".into(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: now,
            joined_at: None,
            last_seen_at: now,
            updated_at: now,
        };
        database.upsert_member(&member).unwrap();
        let plan = CardPlan {
            member,
            original_name: "原名".into(),
            suggested_name: "DH群员0001".into(),
            suffix: "0001".into(),
            status: "planned".into(),
            reason: String::new(),
        };
        assert!(database.enqueue_card_job("a", 1, &plan, true).unwrap());
        let job = database.claim_next_card_job("a").unwrap().unwrap();
        assert_eq!(job.state, "processing");
        assert_eq!(job.attempts, 1);
        drop(database);
        let database = Database::open(&paths).unwrap();
        let job = database.claim_next_card_job("a").unwrap().unwrap();
        assert_eq!(job.state, "processing");
        assert_eq!(job.attempts, 2);
        database.finish_card_job(&job, true, false, "").unwrap();
        let saved = database.list_members("a", 1).unwrap().pop().unwrap();
        // A completed job stores the managed target, while the actual wire
        // card remains owned by the next 旺商聊 roster readback.
        assert_eq!(saved.card_name, "原名");
        assert_eq!(saved.managed_card_name, "DH群员0001");
        assert_eq!(saved.locked_card_name, "DH群员0001");
        assert_eq!(
            database
                .list_card_jobs("a", 1, 20)
                .unwrap()
                .pop()
                .unwrap()
                .state,
            "succeeded"
        );
        assert!(database.enqueue_card_job("a", 1, &plan, false).unwrap());
        assert_eq!(
            database
                .list_card_jobs("a", 1, 20)
                .unwrap()
                .pop()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn card_job_stops_immediately_for_a_non_retryable_member_error() {
        let database = populated_database();
        let now = Utc::now();
        let member = Member {
            account_id: "a".into(),
            group_id: 1,
            user_id: 3,
            nim_id: "nim-3".into(),
            nickname: "已封禁用户".into(),
            card_name: "1".into(),
            original_card_name: "1".into(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: "ACCOUNT_STATE_BAN".into(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: now,
            joined_at: None,
            last_seen_at: now,
            updated_at: now,
        };
        database.upsert_member(&member).unwrap();
        let plan = CardPlan {
            member,
            original_name: "已封禁用户".into(),
            suggested_name: "DH群员0001".into(),
            suffix: "0001".into(),
            status: "planned".into(),
            reason: String::new(),
        };
        database.enqueue_card_job("a", 1, &plan, false).unwrap();
        let job = database.claim_next_card_job("a").unwrap().unwrap();
        database
            .finish_card_job(&job, false, false, "该成员已封禁，跳过自动改名")
            .unwrap();
        let job = database.list_card_jobs("a", 1, 1).unwrap().pop().unwrap();
        assert_eq!(job.state, "failed");
        assert!(job.next_attempt_at.is_none());
        assert_eq!(job.last_error, "该成员已封禁，跳过自动改名");
    }

    #[test]
    fn stored_time_accepts_rfc3339_and_legacy_sqlite_timestamps() {
        let rfc3339 = parse_stored_time("2026-07-31T11:17:42+00:00").unwrap();
        let legacy = parse_stored_time("2026-07-31 11:17:42").unwrap();
        let legacy_fraction = parse_stored_time("2026-07-31 11:17:42.125").unwrap();

        assert_eq!(rfc3339, legacy);
        assert_eq!(legacy_fraction.timestamp_subsec_millis(), 125);
        assert!(parse_stored_time("invalid").is_none());
    }
}
