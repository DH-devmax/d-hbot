use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{AppError, AppResult, InternalError};
use crate::models::{
    Account, ActionRecord, AuditEvent, BatchIngestResult, CardPlan, CardRenameJob, DailySummary,
    EffectOutboxItem, EffectOutboxRequest, EnqueuedEffect, GatewayInboxEvent, GatewayInboxItem,
    Group, GroupAiPermissions, GroupSchedule, KnowledgeBase, KnowledgeDocument, Member, Message,
    ModerationRule, PersistedMessage, TaskItem,
};
use crate::paths::AppPaths;

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
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatus {
    pub path: String,
    pub schema_version: i64,
    pub integrity: String,
    pub accounts: i64,
    pub groups: i64,
    pub messages: i64,
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
                    let _ = result_sender.send(operation(database));
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

    pub async fn upsert_account(&self, account: Account) -> AppResult<()> {
        self.execute(move |database| database.upsert_account(&account))
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

    pub async fn upsert_member(&self, member: Member) -> AppResult<()> {
        self.execute(move |database| database.upsert_member(&member))
            .await
    }

    pub async fn list_members(&self, account_id: String, group_id: i64) -> AppResult<Vec<Member>> {
        self.execute(move |database| database.list_members(&account_id, group_id))
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
        error: String,
    ) -> AppResult<()> {
        self.execute(move |database| database.finish_card_job(&job, success, &error))
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

    pub async fn claim_due_task_reminders(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> AppResult<Vec<TaskItem>> {
        self.execute(move |database| database.claim_due_task_reminders(now, limit))
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
    if old_version > 5 {
        return Err(AppError::new(
            "database_version",
            format!("数据库版本 {old_version} 高于当前程序支持的 v5"),
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
    transaction
        .execute_batch("PRAGMA user_version = 5;")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .commit()
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    Ok(())
}

impl Database {
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
            quick_check_connection(&connection)?;
        }
        let old_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| AppError::new("database_version", error.to_string()))?;
        if database_existed && old_version < 5 {
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
            connection.execute(
                "UPDATE effect_outbox SET state='unknown',next_attempt_at=NULL,claimed_at=NULL,last_error='DH BOT 重启，远端执行结果未知',receipt_json='{\"status\":\"unknown\",\"businessMessage\":\"DH BOT 重启，远端执行结果未知\"}' WHERE state='processing'",
                [],
            )?;
            connection.execute(
                "UPDATE tasks SET reminder_state='retry',reminder_next_attempt_at=NULL,reminder_claimed_at=NULL,reminder_last_error='DH BOT 重启后已恢复提醒' WHERE reminder_state='processing' AND reminder_sent_at IS NULL",
                [],
            )?;
            connection.execute(
                "UPDATE ai_runs SET state='retry',next_retry_at=NULL,last_error='DH BOT 重启后已恢复执行',updated_at=? WHERE state='processing'",
                params![Utc::now().to_rfc3339()],
            )?;
            connection.execute(
                "UPDATE summary_runs SET state='retry',next_retry_at=NULL,last_error='DH BOT 重启后已恢复执行',updated_at=? WHERE state='processing'",
                params![Utc::now().to_rfc3339()],
            )?;
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
                Ok(DatabaseStatus {
                    path: self.path.display().to_string(),
                    schema_version: version,
                    integrity: "ok".into(),
                    accounts,
                    groups,
                    messages,
                })
            })
            .map_err(InternalError::from)?;
        Ok(status)
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

    pub fn set_setting(&self, key: &str, value: &str, sensitive: bool) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO app_settings(key,value,sensitive,updated_at) VALUES(?,?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,sensitive=excluded.sensitive,updated_at=excluded.updated_at", params![key, value, if sensitive { 1 } else { 0 }, Utc::now().to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("settings_write", error.to_string()))
    }

    pub fn upsert_account(&self, account: &Account) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO accounts(id,display_name,role,discovered_at,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,role=excluded.role,updated_at=excluded.updated_at", params![account.id, account.display_name, account.role, account.discovered_at.to_rfc3339(), account.updated_at.to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("account_write", error.to_string()))
    }

    pub fn upsert_group(&self, group: &Group) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO groups(account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,manual_takeover,welcome_message,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET name=excluded.name,owner_user_id=excluded.owner_user_id,updated_at=excluded.updated_at", params![group.account_id, group.group_id, group.name, group.owner_user_id, bool_i(group.enabled), bool_i(group.ai_enabled), bool_i(group.moderation_enabled), bool_i(group.manual_takeover), group.welcome_message, group.updated_at.to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("group_write", error.to_string()))
    }

    pub fn list_groups(&self, account_id: Option<&str>) -> AppResult<Vec<Group>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,manual_takeover,welcome_message,updated_at FROM groups WHERE (?1 IS NULL OR account_id=?1) ORDER BY name,group_id")?;
            let rows = statement.query_map(params![account_id], group_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("groups_read", error.to_string()))
    }

    pub fn upsert_member(&self, member: &Member) -> AppResult<()> {
        self.upsert_member_from_wire(member)
    }

    pub fn upsert_member_from_wire(&self, member: &Member) -> AppResult<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let effective_user_id = resolve_member_identity(&transaction, member)?;
            transaction.execute(
                "INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,user_id) DO UPDATE SET nim_id=CASE WHEN excluded.nim_id<>'' THEN excluded.nim_id ELSE members.nim_id END,nickname=CASE WHEN excluded.nickname<>'' THEN excluded.nickname ELSE members.nickname END,card_name=CASE WHEN excluded.card_name<>'' THEN excluded.card_name ELSE members.card_name END,role=excluded.role,account_state=excluded.account_state,present=excluded.present,join_source=CASE WHEN members.join_source='baseline' AND excluded.join_source<>'baseline' THEN excluded.join_source ELSE members.join_source END,joined_at=COALESCE(members.joined_at,excluded.joined_at),last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",
                params![member.account_id,member.group_id,effective_user_id,member.nim_id,member.nickname,member.card_name,member.original_card_name,member.managed_card_name,member.card_suffix,member.role,member.account_state,bool_i(member.blacklisted),bool_i(member.present),member.join_source,bool_i(member.prompt_read),member.locked_card_name,member.violation_count,member.discovered_at.to_rfc3339(),member.joined_at.map(|value| value.to_rfc3339()),member.last_seen_at.to_rfc3339(),member.updated_at.to_rfc3339()],
            )?;
            transaction.commit()
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

    pub fn mark_members_not_present(
        &self,
        account_id: &str,
        group_id: i64,
        present_user_ids: &[i64],
    ) -> AppResult<Vec<Member>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let current = {
                let mut statement = connection.prepare("SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND present=1")?;
                let result = statement.query_map(params![account_id, group_id], member_from_row)?.collect::<Result<Vec<_>, _>>()?;
                result
            };
            for member in &current {
                if !present_user_ids.contains(&member.user_id) {
                    connection.execute("UPDATE members SET present=0,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?", params![now,account_id,group_id,member.user_id])?;
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
        .map(|_| ())
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
            if existing.as_deref() == Some("succeeded") {
                transaction.commit()?;
                return Ok(false);
            }
            if existing.is_some() {
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
                transaction.execute(
                    "UPDATE members SET card_name=?,managed_card_name=?,locked_card_name=?,card_suffix=?,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?",
                    params![job.desired_name,job.desired_name,job.desired_name,job.suffix,now,job.account_id,job.group_id,job.user_id],
                )?;
            } else {
                let (state, next_retry) = if job.attempts >= 5 {
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
                    "SELECT current.id FROM gateway_inbox AS current WHERE (?1 IS NULL OR current.account_id=?1) AND current.state IN ('pending','retry') AND (current.next_attempt_at IS NULL OR current.next_attempt_at<=?2) AND NOT EXISTS (SELECT 1 FROM gateway_inbox AS prior WHERE prior.account_id=current.account_id AND prior.bridge_session=current.bridge_session AND prior.bridge_sequence>0 AND prior.bridge_sequence<current.bridge_sequence AND prior.state<>'processed') ORDER BY current.received_at,current.id LIMIT ?3",
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
                    "UPDATE gateway_inbox SET state='processed',processed_at=?,claimed_at=NULL,next_attempt_at=NULL,last_error='' WHERE account_id=? AND event_id=? AND state<>'processed'",
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
                if state.as_deref() == Some("processed") {
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
                    "UPDATE gateway_inbox SET state='processed',processed_at=COALESCE(processed_at,?),claimed_at=NULL,next_attempt_at=NULL,last_error='' WHERE account_id=? AND event_id=?",
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
                    "SELECT id FROM effect_outbox WHERE (?1 IS NULL OR account_id=?1) AND state IN ('queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?2) ORDER BY created_at,id LIMIT ?3",
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
                        "SELECT id,account_id,group_id,effect_type,payload_json,dedupe_key,state,attempts,next_attempt_at,last_error,receipt_json,created_at,claimed_at,completed_at FROM effect_outbox WHERE id=?",
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
        let receipt = serde_json::json!({
            "status": "unknown",
            "businessMessage": reason,
        })
        .to_string();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE effect_outbox SET state='unknown',next_attempt_at=NULL,claimed_at=NULL,last_error=?,receipt_json=? WHERE state='processing'",
                params![reason, receipt],
            )
        })
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
                merge_provisional_member(
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

fn merge_provisional_member(
    transaction: &rusqlite::Transaction<'_>,
    account_id: &str,
    group_id: i64,
    provisional_user_id: i64,
    real_user_id: i64,
) -> rusqlite::Result<()> {
    let provisional: Option<Member> = transaction
        .query_row(
            "SELECT account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at FROM members WHERE account_id=? AND group_id=? AND user_id=?",
            params![account_id, group_id, provisional_user_id],
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
            params![real_user_id, account_id, group_id, provisional_user_id],
        )?;
    } else {
        transaction.execute(
            "UPDATE members SET original_card_name=CASE WHEN original_card_name='' THEN ? ELSE original_card_name END,managed_card_name=CASE WHEN managed_card_name='' THEN ? ELSE managed_card_name END,card_suffix=CASE WHEN card_suffix='' THEN ? ELSE card_suffix END,blacklisted=CASE WHEN blacklisted=1 OR ?=1 THEN 1 ELSE 0 END,join_source=CASE WHEN join_source='baseline' AND ?<>'baseline' THEN ? ELSE join_source END,prompt_read=CASE WHEN prompt_read=0 OR ?=0 THEN 0 ELSE 1 END,locked_card_name=CASE WHEN locked_card_name='' THEN ? ELSE locked_card_name END,violation_count=MAX(violation_count,?),discovered_at=CASE WHEN discovered_at='' OR (?<>'' AND ?<discovered_at) THEN ? ELSE discovered_at END,joined_at=CASE WHEN joined_at IS NULL OR (? IS NOT NULL AND ?<joined_at) THEN ? ELSE joined_at END WHERE account_id=? AND group_id=? AND user_id=?",
            params![provisional.original_card_name,provisional.managed_card_name,provisional.card_suffix,bool_i(provisional.blacklisted),provisional.join_source,provisional.join_source,bool_i(provisional.prompt_read),provisional.locked_card_name,provisional.violation_count,provisional.discovered_at.to_rfc3339(),provisional.discovered_at.to_rfc3339(),provisional.discovered_at.to_rfc3339(),provisional.joined_at.map(|value| value.to_rfc3339()),provisional.joined_at.map(|value| value.to_rfc3339()),provisional.joined_at.map(|value| value.to_rfc3339()),account_id,group_id,real_user_id],
        )?;
    }

    transaction.execute(
        "UPDATE messages SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "UPDATE actions SET user_id=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "UPDATE tasks SET assignee_id=? WHERE account_id=? AND group_id=? AND assignee_id=?",
        params![real_user_id, account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "UPDATE tasks SET created_by=? WHERE account_id=? AND group_id=? AND created_by=?",
        params![real_user_id, account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO card_rename_jobs(account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at) SELECT account_id,group_id,?,nim_id,original_name,desired_name,suffix,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at FROM card_rename_jobs WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id, account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "DELETE FROM card_rename_jobs WHERE account_id=? AND group_id=? AND user_id=?",
        params![account_id, group_id, provisional_user_id],
    )?;
    transaction.execute(
        "UPDATE member_identity_aliases SET user_id=?,provisional_user_id=COALESCE(provisional_user_id,?),updated_at=? WHERE account_id=? AND group_id=? AND user_id=?",
        params![real_user_id,provisional_user_id,Utc::now().to_rfc3339(),account_id,group_id,provisional_user_id],
    )?;
    transaction.execute(
        "DELETE FROM members WHERE account_id=? AND group_id=? AND user_id=?",
        params![account_id, group_id, provisional_user_id],
    )?;
    Ok(())
}

fn bool_i(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn parse_time(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn optional_time(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
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
        manual_takeover: row.get::<_, i64>(7)? != 0,
        welcome_message: row.get(8)?,
        updated_at: parse_time(row.get(9)?),
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
        assert_eq!(status.schema_version, 5);
        assert_eq!(status.groups, 0);
        assert_eq!(
            database.get_setting("ai.model").unwrap().as_deref(),
            Some("deepseek-v4-pro")
        );
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
        assert_eq!(database.status().unwrap().schema_version, 5);
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
        assert_eq!(reopened.status().unwrap().schema_version, 5);
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
        assert_eq!(database.status().unwrap().schema_version, 5);
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
                payload_json: "{\"text\":\"hello\"}".into(),
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
        let (state, receipt): (String, String) = executor
            .execute(|database| {
                database
                    .with_connection(|connection| {
                        connection.query_row(
                            "SELECT state,receipt_json FROM effect_outbox WHERE dedupe_key='shutdown-unknown'",
                            [],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                    })
                    .map_err(|error| AppError::new("test_query", error.to_string()))
            })
            .await
            .unwrap();
        assert_eq!(state, "unknown");
        assert!(receipt.contains("退出等待超时"));
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
        database.finish_card_job(&job, true, "").unwrap();
        let saved = database.list_members("a", 1).unwrap().pop().unwrap();
        assert_eq!(saved.card_name, "DH群员0001");
        assert_eq!(
            database
                .list_card_jobs("a", 1, 20)
                .unwrap()
                .pop()
                .unwrap()
                .state,
            "succeeded"
        );
    }
}
