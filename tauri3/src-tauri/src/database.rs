use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{AppError, AppResult, InternalError};
use crate::models::{
    Account, CardPlan, CardRenameJob, Group, KnowledgeBase, KnowledgeDocument, Member, Message,
    PersistedMessage,
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
  UNIQUE(account_id, group_id, server_message_id),
  FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
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
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(base_id, content_hash),
  FOREIGN KEY(base_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE
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

#[derive(Clone)]
pub struct Database {
    pub(crate) path: PathBuf,
    pub(crate) connection: Arc<Mutex<Connection>>,
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

fn migrate_schema(connection: &mut Connection, old_version: i64) -> AppResult<()> {
    if old_version > 3 {
        return Err(AppError::new(
            "database_version",
            format!("数据库版本 {old_version} 高于当前程序支持的 v3"),
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
        ("actions", "dedupe_key", "TEXT NOT NULL DEFAULT ''"),
        ("tasks", "assignee_id", "INTEGER NOT NULL DEFAULT 0"),
        ("tasks", "created_by", "INTEGER NOT NULL DEFAULT 0"),
        ("tasks", "reminder_at", "TEXT"),
        ("tasks", "reminder_sent_at", "TEXT"),
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
        .execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS actions_dedupe_key_idx ON actions(account_id, dedupe_key) WHERE dedupe_key <> ''")
        .map_err(|error| AppError::new("database_migration", error.to_string()))?;
    transaction
        .execute_batch("PRAGMA user_version = 3;")
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
        let old_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| AppError::new("database_version", error.to_string()))?;
        if database_existed && old_version < 3 {
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
        let result: String = self
            .with_connection(|connection| {
                connection.query_row("PRAGMA quick_check", [], |row| row.get(0))
            })
            .map_err(InternalError::from)?;
        if result != "ok" {
            return Err(AppError::new(
                "database_corrupt",
                format!("SQLite 完整性检查失败：{result}"),
            ));
        }
        Ok(())
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
        self.with_connection(|connection| connection.execute("INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,original_card_name,managed_card_name,card_suffix,role,account_state,blacklisted,present,join_source,prompt_read,locked_card_name,violation_count,discovered_at,joined_at,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,user_id) DO UPDATE SET nim_id=excluded.nim_id,nickname=excluded.nickname,card_name=excluded.card_name,role=excluded.role,account_state=excluded.account_state,blacklisted=excluded.blacklisted,present=excluded.present,join_source=CASE WHEN members.join_source='baseline' AND excluded.join_source<>'baseline' THEN excluded.join_source ELSE members.join_source END,prompt_read=excluded.prompt_read,locked_card_name=CASE WHEN excluded.locked_card_name<>'' THEN excluded.locked_card_name ELSE members.locked_card_name END,violation_count=excluded.violation_count,joined_at=COALESCE(members.joined_at,excluded.joined_at),last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at", params![member.account_id,member.group_id,member.user_id,member.nim_id,member.nickname,member.card_name,member.original_card_name,member.managed_card_name,member.card_suffix,member.role,member.account_state,bool_i(member.blacklisted),bool_i(member.present),member.join_source,bool_i(member.prompt_read),member.locked_card_name,member.violation_count,member.discovered_at.to_rfc3339(),member.joined_at.map(|value| value.to_rfc3339()),member.last_seen_at.to_rfc3339(),member.updated_at.to_rfc3339()]))
            .map(|_| ())
            .map_err(|error| AppError::new("member_write", error.to_string()))
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

    pub fn insert_message(&self, message: &Message) -> AppResult<PersistedMessage> {
        self.with_connection(|connection| {
            let changed = connection.execute("INSERT OR IGNORE INTO messages(account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![message.account_id,message.group_id,message.server_message_id,message.sequence,message.user_id,message.sender_name,message.kind,message.text,message.sent_at.to_rfc3339(),message.received_at.to_rfc3339(),message.processed_at.map(|v| v.to_rfc3339()),message.acknowledged_at.map(|v| v.to_rfc3339()),message.processing_state,message.attempts,message.next_attempt_at.map(|value| value.to_rfc3339()),message.last_error])?;
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
            let mut statement = connection.prepare("SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error FROM messages WHERE account_id=? AND processed_at IS NULL AND processing_state IN ('pending','queued','retry') AND (next_attempt_at IS NULL OR next_attempt_at<=?) ORDER BY sequence,id LIMIT ?")?;
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
            let mut statement = connection.prepare("SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id JOIN knowledge_base_groups g ON g.base_id=b.id AND g.account_id=b.account_id WHERE b.account_id=? AND g.group_id=? AND b.enabled=1 AND g.enabled=1 ORDER BY b.name,d.title,d.id")?;
            let rows = statement.query_map(params![account_id, group_id], |row| Ok(KnowledgeDocument { id: row.get(0)?, base_id: row.get(1)?, base_name: row.get(2)?, title: row.get(3)?, kind: row.get(4)?, content: row.get(5)?, source: row.get(6)?, content_hash: row.get(7)? }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("knowledge_read", error.to_string()))
    }
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
        assert_eq!(status.schema_version, 3);
        assert_eq!(status.groups, 0);
        assert_eq!(
            database.get_setting("ai.model").unwrap().as_deref(),
            Some("deepseek-v4-pro")
        );
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
        assert_eq!(database.status().unwrap().schema_version, 3);
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
        assert_eq!(
            std::fs::read_dir(paths.v3.join("backups")).unwrap().count(),
            1
        );
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
        };
        let first = database.insert_message(&message).unwrap();
        assert!(first.inserted);
        assert!(first.id > 0);
        let second = database.insert_message(&message).unwrap();
        assert!(!second.inserted);
        assert_eq!(second.id, first.id);
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
        };
        let persisted = database.insert_message(&message).unwrap();
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
