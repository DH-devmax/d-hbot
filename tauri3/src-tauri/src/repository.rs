use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::database::Database;
use crate::error::{AppError, AppResult};
use crate::models::{
    ActionRecord, AuditEvent, DailySummary, GroupAiPermissions, GroupSchedule, KnowledgeBase,
    KnowledgeBinding, KnowledgeDocument, Message, ModerationRule, RuleAction, ScheduleRun,
    TaskItem,
};

impl Database {
    pub fn group_ai_permissions(
        &self,
        account_id: &str,
        group_id: i64,
    ) -> AppResult<Option<GroupAiPermissions>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT account_id,group_id,reply,tasks,recall,mute,remove_member FROM group_ai_permissions WHERE account_id=? AND group_id=?",
                    params![account_id, group_id],
                    |row| {
                        Ok(GroupAiPermissions {
                            account_id: row.get(0)?,
                            group_id: row.get(1)?,
                            reply: row.get::<_, i64>(2)? != 0,
                            tasks: row.get::<_, i64>(3)? != 0,
                            recall: row.get::<_, i64>(4)? != 0,
                            mute: row.get::<_, i64>(5)? != 0,
                            remove: row.get::<_, i64>(6)? != 0,
                        })
                    },
                )
                .optional()
        })
        .map_err(|error| AppError::new("ai_permissions_read", error.to_string()))
    }

    pub fn save_group_ai_permissions(&self, value: &GroupAiPermissions) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO group_ai_permissions(account_id,group_id,reply,tasks,recall,mute,remove_member,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET reply=excluded.reply,tasks=excluded.tasks,recall=excluded.recall,mute=excluded.mute,remove_member=excluded.remove_member,updated_at=excluded.updated_at",
                params![
                    value.account_id,
                    value.group_id,
                    bool_i(value.reply),
                    bool_i(value.tasks),
                    bool_i(value.recall),
                    bool_i(value.mute),
                    bool_i(value.remove),
                    Utc::now().to_rfc3339()
                ],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("ai_permissions_write", error.to_string()))
    }

    pub fn set_group_features(
        &self,
        account_id: &str,
        group_id: i64,
        enabled: bool,
        ai_enabled: bool,
        moderation_enabled: bool,
        manual_takeover: bool,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("UPDATE groups SET enabled=?,ai_enabled=?,moderation_enabled=?,manual_takeover=?,updated_at=? WHERE account_id=? AND group_id=?", params![bool_i(enabled),bool_i(ai_enabled),bool_i(moderation_enabled),bool_i(manual_takeover),Utc::now().to_rfc3339(),account_id,group_id]))
            .and_then(|changed| if changed == 1 { Ok(()) } else { Err(rusqlite::Error::QueryReturnedNoRows) })
            .map_err(|error| AppError::new("group_update", error.to_string()))
    }

    pub fn set_group_welcome(
        &self,
        account_id: &str,
        group_id: i64,
        welcome_message: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE groups SET welcome_message=?,updated_at=? WHERE account_id=? AND group_id=?",
                params![welcome_message.trim(), Utc::now().to_rfc3339(), account_id, group_id],
            )
        })
        .and_then(|changed| {
            if changed == 1 {
                Ok(())
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
        .map_err(|error| AppError::new("group_update", error.to_string()))
    }

    pub fn list_messages(
        &self,
        account_id: &str,
        group_id: Option<i64>,
        limit: usize,
    ) -> AppResult<Vec<Message>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error FROM messages WHERE account_id=? AND (?2 IS NULL OR group_id=?2) ORDER BY received_at DESC,id DESC LIMIT ?3")?;
            let rows = statement.query_map(params![account_id, group_id, limit.clamp(1, 1000) as i64], |row| Ok(Message { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, server_message_id: row.get(3)?, sequence: row.get(4)?, user_id: row.get(5)?, sender_name: row.get(6)?, kind: row.get(7)?, text: row.get(8)?, sent_at: parse_time(row.get::<_, String>(9)?), received_at: parse_time(row.get::<_, String>(10)?), processed_at: optional_time(row.get(11)?), acknowledged_at: optional_time(row.get(12)?), processing_state: row.get(13)?, attempts: row.get(14)?, next_attempt_at: optional_time(row.get(15)?), last_error: row.get(16)? }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("messages_read", error.to_string()))
    }

    pub fn mark_message_processed(&self, id: i64, acknowledged: bool) -> AppResult<()> {
        if acknowledged {
            self.mark_message_acknowledged(id)?;
        }
        self.finish_message_processing(id, true, "")
    }

    pub fn recent_messages(
        &self,
        account_id: &str,
        group_id: i64,
        limit: usize,
    ) -> AppResult<Vec<Message>> {
        let mut messages = self.list_messages(account_id, Some(group_id), limit)?;
        messages.sort_by_key(|message| message.received_at);
        Ok(messages)
    }

    pub fn record_rule_runtime(
        &self,
        rule_id: i64,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
        window_seconds: i64,
    ) -> AppResult<bool> {
        self.with_connection(|connection| {
            let existing: Option<(Option<String>, i64, Option<String>)> = connection.query_row(
                "SELECT window_started_at,window_count,last_executed_at FROM rule_runtime_state WHERE rule_id=? AND account_id=? AND group_id=? AND user_id=?",
                params![rule_id, account_id, group_id, user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            let cooldown_blocked = existing.as_ref().and_then(|(_, _, last)| last.as_deref()).and_then(|last| chrono::DateTime::parse_from_rfc3339(last).ok()).map(|last| now.signed_duration_since(last.with_timezone(&Utc)).num_seconds() < window_seconds).unwrap_or(false);
            if cooldown_blocked {
                return Ok(false);
            }
            connection.execute(
                "INSERT INTO rule_runtime_state(rule_id,account_id,group_id,user_id,window_started_at,window_count,last_executed_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(rule_id,account_id,group_id,user_id) DO UPDATE SET window_started_at=excluded.window_started_at,window_count=rule_runtime_state.window_count+1,last_executed_at=excluded.last_executed_at,updated_at=excluded.updated_at",
                params![rule_id, account_id, group_id, user_id, now.to_rfc3339(), 1_i64, now.to_rfc3339(), now.to_rfc3339()],
            )?;
            Ok(true)
        }).map_err(|error| AppError::new("rule_runtime", error.to_string()))
    }

    pub fn list_rules(
        &self,
        account_id: &str,
        group_id: Option<i64>,
    ) -> AppResult<Vec<ModerationRule>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json FROM rules WHERE account_id=? AND (?2 IS NULL OR group_id=0 OR group_id=?2) ORDER BY priority DESC,id")?;
            let mut rules = statement.query_map(params![account_id, group_id], |row| {
                let roles: String = row.get(14)?; let users: String = row.get(15)?;
                Ok(ModerationRule { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, name: row.get(3)?, matcher: row.get(4)?, pattern: row.get(5)?, threshold: row.get(6)?, count: row.get(7)?, window_seconds: row.get(8)?, cooldown_seconds: row.get(9)?, priority: row.get(10)?, mode: row.get(11)?, enabled: row.get::<_, i64>(12)? != 0, semantic_threshold: row.get(13)?, exempt_roles: serde_json::from_str(&roles).unwrap_or_default(), exempt_user_ids: serde_json::from_str(&users).unwrap_or_default(), actions: Vec::new() })
            })?.collect::<Result<Vec<_>, _>>()?;
            for rule in &mut rules {
                let mut action_statement = connection.prepare("SELECT kind,duration_seconds,message FROM rule_actions WHERE rule_id=? ORDER BY position,id")?;
                rule.actions = action_statement.query_map(params![rule.id], |row| Ok(RuleAction { kind: row.get(0)?, duration_seconds: row.get(1)?, message: row.get(2)? }))?.collect::<Result<Vec<_>, _>>()?;
            }
            Ok(rules)
        }).map_err(|error| AppError::new("rules_read", error.to_string()))
    }

    pub fn save_rule(&self, rule: &ModerationRule) -> AppResult<i64> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let now = Utc::now().to_rfc3339();
            let roles = serde_json::to_string(&rule.exempt_roles).unwrap_or_else(|_| "[]".into());
            let users = serde_json::to_string(&rule.exempt_user_ids).unwrap_or_else(|_| "[]".into());
            let id = if rule.id > 0 {
                transaction.execute("UPDATE rules SET group_id=?,name=?,matcher=?,pattern=?,threshold=?,count=?,window_seconds=?,cooldown_seconds=?,priority=?,mode=?,enabled=?,semantic_threshold=?,exempt_roles_json=?,exempt_user_ids_json=?,updated_at=? WHERE id=? AND account_id=?", params![rule.group_id,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,rule.cooldown_seconds,rule.priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,roles,users,now,rule.id,rule.account_id])?;
                rule.id
            } else {
                transaction.execute("INSERT INTO rules(account_id,group_id,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![rule.account_id,rule.group_id,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,rule.cooldown_seconds,rule.priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,roles,users,now,now])?;
                transaction.last_insert_rowid()
            };
            transaction.execute("DELETE FROM rule_actions WHERE rule_id=?", params![id])?;
            for (position, action) in rule.actions.iter().enumerate() { transaction.execute("INSERT INTO rule_actions(rule_id,kind,duration_seconds,message,position) VALUES(?,?,?,?,?)", params![id,action.kind,action.duration_seconds,action.message,position as i64])?; }
            transaction.commit()?;
            Ok(id)
        }).map_err(|error| AppError::new("rule_write", error.to_string()))
    }

    pub fn import_rules(&self, account_id: &str, rules: &[ModerationRule]) -> AppResult<usize> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let now = Utc::now().to_rfc3339();
            for rule in rules {
                let roles = serde_json::to_string(&rule.exempt_roles).unwrap_or_else(|_| "[]".into());
                let users = serde_json::to_string(&rule.exempt_user_ids).unwrap_or_else(|_| "[]".into());
                transaction.execute("INSERT INTO rules(account_id,group_id,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![account_id,rule.group_id,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,rule.cooldown_seconds,rule.priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,roles,users,now,now])?;
                let rule_id = transaction.last_insert_rowid();
                for (position, action) in rule.actions.iter().enumerate() {
                    transaction.execute("INSERT INTO rule_actions(rule_id,kind,duration_seconds,message,position) VALUES(?,?,?,?,?)", params![rule_id, action.kind, action.duration_seconds, action.message, position as i64])?;
                }
            }
            transaction.commit()?;
            Ok(rules.len())
        }).map_err(|error| AppError::new("rules_import", error.to_string()))
    }

    pub fn delete_rule(&self, account_id: &str, rule_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM rules WHERE id=? AND account_id=?",
                params![rule_id, account_id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("rule_delete", error.to_string()))
    }

    pub fn create_knowledge_base(&self, base: &KnowledgeBase) -> AppResult<i64> {
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            connection.execute("INSERT INTO knowledge_bases(account_id,name,description,enabled,built_in,read_only,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)", params![base.account_id,base.name,base.description,bool_i(base.enabled),bool_i(base.built_in),bool_i(base.read_only),now,now])?;
            Ok(connection.last_insert_rowid())
        }).map_err(|error| AppError::new("knowledge_write", error.to_string()))
    }

    pub fn knowledge_base_account(&self, base_id: i64) -> AppResult<String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT account_id FROM knowledge_bases WHERE id=?",
                params![base_id],
                |row| row.get(0),
            )
        })
        .map_err(|error| AppError::new("knowledge_read", error.to_string()))
    }

    pub fn update_knowledge_base(&self, base: &KnowledgeBase) -> AppResult<()> {
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE knowledge_bases SET name=?,description=?,enabled=?,updated_at=? WHERE id=? AND account_id=? AND read_only=0",
                params![base.name.trim(), base.description.trim(), bool_i(base.enabled), Utc::now().to_rfc3339(), base.id, base.account_id],
            )?;
            if changed == 1 { Ok(()) } else { Err(rusqlite::Error::QueryReturnedNoRows) }
        }).map_err(|error| AppError::new("knowledge_update", error.to_string()))
    }

    pub fn clone_knowledge_base(
        &self,
        account_id: &str,
        base_id: i64,
        name: &str,
    ) -> AppResult<i64> {
        if name.trim().is_empty() {
            return Err(AppError::new("knowledge_name", "知识库名称不能为空"));
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let source: (String, String) = transaction.query_row(
                "SELECT description,account_id FROM knowledge_bases WHERE id=? AND account_id=?",
                params![base_id, account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let now = Utc::now().to_rfc3339();
            transaction.execute(
                "INSERT INTO knowledge_bases(account_id,name,description,enabled,built_in,read_only,created_at,updated_at) VALUES(?,?,?,1,0,0,?,?)",
                params![source.1, name.trim(), source.0, now, now],
            )?;
            let new_id = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO knowledge_documents(base_id,title,kind,content,source,content_hash,created_at,updated_at) SELECT ?,title,kind,content,source,content_hash,created_at,updated_at FROM knowledge_documents WHERE base_id=?",
                params![new_id, base_id],
            )?;
            transaction.commit()?;
            Ok(new_id)
        }).map_err(|error| AppError::new("knowledge_clone", error.to_string()))
    }

    pub fn delete_knowledge_base(&self, account_id: &str, base_id: i64) -> AppResult<()> {
        let read_only = self
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT read_only FROM knowledge_bases WHERE id=? AND account_id=?",
                        params![base_id, account_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
            })
            .map_err(|error| AppError::new("knowledge_delete", error.to_string()))?;
        if read_only == Some(1) {
            return Err(AppError::new(
                "knowledge_read_only",
                "内置只读知识库不能删除，请先复制为可编辑知识库",
            ));
        }
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM knowledge_bases WHERE id=? AND account_id=?",
                params![base_id, account_id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("knowledge_delete", error.to_string()))
    }

    pub fn upsert_knowledge_document(&self, document: &KnowledgeDocument) -> AppResult<i64> {
        let hash = if document.content_hash.is_empty() {
            format!("{:x}", Sha256::digest(document.content.as_bytes()))
        } else {
            document.content_hash.clone()
        };
        self.with_connection(|connection| {
            let read_only: Option<i64> = connection.query_row(
                "SELECT read_only FROM knowledge_bases WHERE id=?",
                params![document.base_id],
                |row| row.get(0),
            ).optional()?;
            if read_only == Some(1) {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let now = Utc::now().to_rfc3339();
            if document.id > 0 {
                connection.execute("UPDATE knowledge_documents SET title=?,kind=?,content=?,source=?,content_hash=?,updated_at=? WHERE id=? AND base_id=?", params![document.title,document.kind,document.content,document.source,hash,now,document.id,document.base_id])?;
                Ok(document.id)
            } else {
                connection.execute("INSERT INTO knowledge_documents(base_id,title,kind,content,source,content_hash,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(base_id,content_hash) DO UPDATE SET title=excluded.title,kind=excluded.kind,source=excluded.source,updated_at=excluded.updated_at", params![document.base_id,document.title,document.kind,document.content,document.source,hash,now,now])?;
                connection.query_row("SELECT id FROM knowledge_documents WHERE base_id=? AND content_hash=?", params![document.base_id,hash], |row| row.get(0))
            }
        }).map_err(|error| AppError::new("knowledge_write", error.to_string()))
    }

    pub fn delete_knowledge_document(&self, base_id: i64, document_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            let read_only: Option<i64> = connection.query_row(
                "SELECT b.read_only FROM knowledge_bases b JOIN knowledge_documents d ON d.base_id=b.id WHERE b.id=? AND d.id=?",
                params![base_id, document_id], |row| row.get(0),
            ).optional()?;
            if read_only == Some(1) { return Err(rusqlite::Error::InvalidQuery); }
            connection.execute("DELETE FROM knowledge_documents WHERE id=? AND base_id=?", params![document_id, base_id])
        }).map(|_| ()).map_err(|error| AppError::new("knowledge_document_delete", error.to_string()))
    }

    pub fn list_knowledge_documents(&self, base_id: i64) -> AppResult<Vec<KnowledgeDocument>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id WHERE d.base_id=? ORDER BY d.title,d.id",
            )?;
            let rows = statement.query_map(params![base_id], |row| {
                Ok(KnowledgeDocument {
                    id: row.get(0)?,
                    base_id: row.get(1)?,
                    base_name: row.get(2)?,
                    title: row.get(3)?,
                    kind: row.get(4)?,
                    content: row.get(5)?,
                    source: row.get(6)?,
                    content_hash: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("knowledge_documents_read", error.to_string()))
    }

    pub fn bind_knowledge_base(
        &self,
        base_id: i64,
        account_id: &str,
        group_ids: &[i64],
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM knowledge_base_groups WHERE base_id=? AND account_id=?", params![base_id,account_id])?;
            for group_id in group_ids { transaction.execute("INSERT INTO knowledge_base_groups(base_id,account_id,group_id,enabled) VALUES(?,?,?,1)", params![base_id,account_id,group_id])?; }
            transaction.commit()
        }).map_err(|error| AppError::new("knowledge_binding", error.to_string()))
    }

    pub fn list_knowledge_bindings(
        &self,
        account_id: &str,
        base_id: Option<i64>,
    ) -> AppResult<Vec<KnowledgeBinding>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT base_id,group_id,enabled FROM knowledge_base_groups WHERE account_id=? AND (?2 IS NULL OR base_id=?2) ORDER BY base_id,group_id")?;
            let rows = statement.query_map(params![account_id, base_id], |row| Ok(KnowledgeBinding { base_id: row.get(0)?, account_id: account_id.to_string(), group_id: row.get(1)?, enabled: row.get::<_, i64>(2)? != 0 }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("knowledge_bindings_read", error.to_string()))
    }

    pub fn list_tasks(&self, account_id: &str, group_id: Option<i64>) -> AppResult<Vec<TaskItem>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,reminder_sent_at,created_at,updated_at FROM tasks WHERE account_id=? AND (?2 IS NULL OR group_id=?2) ORDER BY CASE status WHEN 'pending' THEN 0 ELSE 1 END,due_at,id")?;
            let rows = statement.query_map(params![account_id, group_id], |row| Ok(TaskItem { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, title: row.get(3)?, description: row.get(4)?, status: row.get(5)?, assignee_id: row.get(6)?, created_by: row.get(7)?, due_at: optional_time(row.get(8)?), reminder_at: optional_time(row.get(9)?), reminder_sent_at: optional_time(row.get(10)?), created_at: parse_time(row.get(11)?), updated_at: parse_time(row.get(12)?) }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("tasks_read", error.to_string()))
    }

    pub fn save_task(&self, task: &TaskItem) -> AppResult<i64> {
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            if task.id > 0 { connection.execute("UPDATE tasks SET title=?,description=?,status=?,assignee_id=?,created_by=?,due_at=?,reminder_at=?,updated_at=? WHERE id=? AND account_id=?", params![task.title,task.description,task.status,task.assignee_id,task.created_by,task.due_at.map(|value| value.to_rfc3339()),task.reminder_at.map(|value| value.to_rfc3339()),now,task.id,task.account_id])?; Ok(task.id) }
            else { connection.execute("INSERT INTO tasks(account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)", params![task.account_id,task.group_id,task.title,task.description,task.status,task.assignee_id,task.created_by,task.due_at.map(|value| value.to_rfc3339()),task.reminder_at.map(|value| value.to_rfc3339()),now,now])?; Ok(connection.last_insert_rowid()) }
        }).map_err(|error| AppError::new("task_write", error.to_string()))
    }

    pub fn claim_due_task_reminders(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> AppResult<Vec<TaskItem>> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let rows = {
                let mut statement = tx.prepare("SELECT id,account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,reminder_sent_at,created_at,updated_at FROM tasks WHERE reminder_at IS NOT NULL AND reminder_at<=? AND reminder_sent_at IS NULL AND status!='done' ORDER BY reminder_at,id LIMIT ?")?;
                let result = statement.query_map(params![now.to_rfc3339(), limit.clamp(1, 100) as i64], |row| Ok(TaskItem { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, title: row.get(3)?, description: row.get(4)?, status: row.get(5)?, assignee_id: row.get(6)?, created_by: row.get(7)?, due_at: optional_time(row.get(8)?), reminder_at: optional_time(row.get(9)?), reminder_sent_at: optional_time(row.get(10)?), created_at: parse_time(row.get(11)?), updated_at: parse_time(row.get(12)?) }))?.collect::<Result<Vec<_>, _>>()?;
                result
            };
            for task in &rows { tx.execute("UPDATE tasks SET reminder_sent_at=? WHERE id=? AND reminder_sent_at IS NULL", params![now.to_rfc3339(), task.id])?; }
            tx.commit()?;
            Ok(rows)
        }).map_err(|error| AppError::new("task_reminder_claim", error.to_string()))
    }

    pub fn delete_task(&self, account_id: &str, task_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM tasks WHERE id=? AND account_id=?",
                params![task_id, account_id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("task_delete", error.to_string()))
    }

    pub fn save_daily_summary(&self, summary: &DailySummary) -> AppResult<i64> {
        self.with_connection(|connection| {
            connection.execute("INSERT INTO daily_summaries(account_id,group_id,local_date,content,source,created_at) VALUES(?,?,?,?,?,?) ON CONFLICT(account_id,group_id,local_date) DO UPDATE SET content=excluded.content,source=excluded.source,created_at=excluded.created_at",params![summary.account_id,summary.group_id,summary.local_date,summary.content,summary.source,summary.created_at.to_rfc3339()])?;
            connection.query_row("SELECT id FROM daily_summaries WHERE account_id=? AND group_id=? AND local_date=?",params![summary.account_id,summary.group_id,summary.local_date],|row|row.get(0))
        }).map_err(|error|AppError::new("summary_write",error.to_string()))
    }

    pub fn list_daily_summaries(
        &self,
        account_id: &str,
        limit: usize,
    ) -> AppResult<Vec<DailySummary>> {
        self.with_connection(|connection|{let mut statement=connection.prepare("SELECT id,account_id,group_id,local_date,content,source,created_at FROM daily_summaries WHERE account_id=? ORDER BY local_date DESC,id DESC LIMIT ?")?;let rows=statement.query_map(params![account_id,limit.clamp(1,100)as i64],|row|Ok(DailySummary{id:row.get(0)?,account_id:row.get(1)?,group_id:row.get(2)?,local_date:row.get(3)?,content:row.get(4)?,source:row.get(5)?,created_at:parse_time(row.get(6)?)}))?;rows.collect::<Result<Vec<_>,_>>()}).map_err(|error|AppError::new("summary_read",error.to_string()))
    }

    pub fn list_schedule_runs(
        &self,
        account_id: &str,
        schedule_id: Option<i64>,
        limit: usize,
    ) -> AppResult<Vec<ScheduleRun>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,schedule_id,group_id,local_date,action,run_key,success,error,attempts,next_retry_at,created_at FROM schedule_runs WHERE account_id=? AND (?2 IS NULL OR schedule_id=?2) ORDER BY created_at DESC,id DESC LIMIT ?3")?;
            let rows = statement.query_map(params![account_id, schedule_id, limit.clamp(1, 500) as i64], |row| Ok(ScheduleRun { id: row.get(0)?, schedule_id: row.get(1)?, account_id: account_id.to_string(), group_id: row.get(2)?, local_date: row.get(3)?, action: row.get(4)?, run_key: row.get(5)?, success: row.get::<_, i64>(6)? != 0, error: row.get(7)?, attempts: row.get(8)?, next_retry_at: optional_time(row.get(9)?), created_at: parse_time(row.get(10)?) }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("schedule_runs_read", error.to_string()))
    }

    pub fn list_schedules(&self, account_id: &str) -> AppResult<Vec<GroupSchedule>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,name,enabled,open_time,close_time,timezone FROM group_schedules WHERE account_id=? ORDER BY name,id")?;
            let mut schedules = statement.query_map(params![account_id], |row| Ok(GroupSchedule { id: row.get(0)?, account_id: row.get(1)?, name: row.get(2)?, enabled: row.get::<_,i64>(3)? != 0, open_time: row.get(4)?, close_time: row.get(5)?, timezone: row.get(6)?, group_ids: Vec::new() }))?.collect::<Result<Vec<_>, _>>()?;
            for schedule in &mut schedules { let mut bindings = connection.prepare("SELECT group_id FROM group_schedule_groups WHERE schedule_id=? ORDER BY group_id")?; schedule.group_ids = bindings.query_map(params![schedule.id], |row| row.get(0))?.collect::<Result<Vec<_>,_>>()?; }
            Ok(schedules)
        }).map_err(|error| AppError::new("schedules_read", error.to_string()))
    }

    pub fn save_schedule(&self, schedule: &GroupSchedule) -> AppResult<i64> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?; let now = Utc::now().to_rfc3339();
            let id = if schedule.id > 0 { transaction.execute("UPDATE group_schedules SET name=?,enabled=?,open_time=?,close_time=?,timezone=?,updated_at=? WHERE id=? AND account_id=?", params![schedule.name,bool_i(schedule.enabled),schedule.open_time,schedule.close_time,schedule.timezone,now,schedule.id,schedule.account_id])?; schedule.id } else { transaction.execute("INSERT INTO group_schedules(account_id,name,enabled,open_time,close_time,timezone,updated_at) VALUES(?,?,?,?,?,?,?)", params![schedule.account_id,schedule.name,bool_i(schedule.enabled),schedule.open_time,schedule.close_time,schedule.timezone,now])?; transaction.last_insert_rowid() };
            transaction.execute("DELETE FROM group_schedule_groups WHERE schedule_id=?", params![id])?;
            for group_id in &schedule.group_ids { transaction.execute("INSERT INTO group_schedule_groups(schedule_id,account_id,group_id) VALUES(?,?,?)", params![id,schedule.account_id,group_id])?; }
            transaction.commit()?; Ok(id)
        }).map_err(|error| AppError::new("schedule_write", error.to_string()))
    }

    pub fn delete_schedule(&self, account_id: &str, schedule_id: i64) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM group_schedules WHERE id=? AND account_id=?",
                params![schedule_id, account_id],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("schedule_delete", error.to_string()))
    }

    pub fn claim_schedule_run(
        &self,
        schedule_id: i64,
        account_id: &str,
        group_id: i64,
        action: &str,
        run_key: &str,
    ) -> AppResult<bool> {
        let local_date = Utc::now().format("%Y-%m-%d").to_string();
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute("INSERT OR IGNORE INTO schedule_runs(schedule_id,account_id,group_id,local_date,action,run_key,success,error,attempts,next_retry_at,created_at) VALUES(?,?,?,?,?,?,0,'',1,NULL,?)", params![schedule_id,account_id,group_id,local_date,action,run_key,now])?;
            if changed == 1 { transaction.commit()?; return Ok(true); }
            let existing: Option<(i64,i64,Option<String>)> = transaction.query_row("SELECT success,attempts,next_retry_at FROM schedule_runs WHERE run_key=?", params![run_key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let claim = existing.map(|(success,attempts,next_retry)| success == 0 && attempts < 5 && next_retry.as_deref().map(|value| value <= now.as_str()).unwrap_or(false)).unwrap_or(false);
            if claim { transaction.execute("UPDATE schedule_runs SET attempts=attempts+1,next_retry_at=NULL WHERE run_key=?", params![run_key])?; }
            transaction.commit()?;
            Ok(claim)
        }).map_err(|error| AppError::new("schedule_run", error.to_string()))
    }

    pub fn finish_schedule_run(&self, run_key: &str, success: bool, error: &str) -> AppResult<()> {
        let next_retry = if success {
            None
        } else {
            Some((Utc::now() + chrono::Duration::seconds(30)).to_rfc3339())
        };
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE schedule_runs SET success=?,error=?,next_retry_at=? WHERE run_key=?",
                params![bool_i(success), error, next_retry, run_key],
            )
        })
        .map(|_| ())
        .map_err(|error| AppError::new("schedule_run", error.to_string()))
    }

    pub fn record_action(&self, action: &ActionRecord) -> AppResult<i64> {
        self.with_connection(|connection| {
            connection.execute("INSERT INTO actions(account_id,group_id,user_id,message_id,rule_id,kind,mode,duration_seconds,reason,success,error,dedupe_key,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,dedupe_key) WHERE dedupe_key<>'' DO UPDATE SET success=excluded.success,error=excluded.error,reason=excluded.reason,created_at=excluded.created_at", params![action.account_id,action.group_id,action.user_id,action.message_id,action.rule_id,action.kind,action.mode,action.duration_seconds,action.reason,bool_i(action.success),action.error,action.dedupe_key,action.created_at.to_rfc3339()])?;
            if action.dedupe_key.is_empty() {
                Ok(connection.last_insert_rowid())
            } else {
                connection.query_row("SELECT id FROM actions WHERE account_id=? AND dedupe_key=?", params![action.account_id,action.dedupe_key], |row| row.get(0))
            }
        }).map_err(|error| AppError::new("action_write", error.to_string()))
    }

    pub fn action_succeeded(&self, account_id: &str, dedupe_key: &str) -> AppResult<bool> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT success FROM actions WHERE account_id=? AND dedupe_key=?",
                    params![account_id, dedupe_key],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        })
        .map(|value| value == Some(1))
        .map_err(|error| AppError::new("action_read", error.to_string()))
    }

    pub fn record_audit(&self, event: &AuditEvent) -> AppResult<i64> {
        self.with_connection(|connection| { connection.execute("INSERT INTO audit_events(account_id,group_id,user_id,actor,event,level,details,created_at) VALUES(?,?,?,?,?,?,?,?)", params![event.account_id,event.group_id,event.user_id,event.actor,event.event,event.level,event.details,event.created_at.to_rfc3339()])?; Ok(connection.last_insert_rowid()) }).map_err(|error| AppError::new("audit_write", error.to_string()))
    }

    pub fn list_audit(&self, account_id: &str, limit: usize) -> AppResult<Vec<AuditEvent>> {
        self.with_connection(|connection| { let mut statement = connection.prepare("SELECT id,account_id,group_id,user_id,actor,event,level,details,created_at FROM audit_events WHERE account_id=? ORDER BY created_at DESC,id DESC LIMIT ?")?; let rows = statement.query_map(params![account_id,limit.clamp(1,1000) as i64], |row| Ok(AuditEvent { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, user_id: row.get(3)?, actor: row.get(4)?, event: row.get(5)?, level: row.get(6)?, details: row.get(7)?, created_at: parse_time(row.get(8)?) }))?; rows.collect::<Result<Vec<_>,_>>() }).map_err(|error| AppError::new("audit_read", error.to_string()))
    }

    pub(crate) fn query_audit(&self, query: &crate::AuditQuery) -> AppResult<Vec<AuditEvent>> {
        self.with_connection(|connection| {
            let mut sql = String::from("SELECT id,account_id,group_id,user_id,actor,event,level,details,created_at FROM audit_events WHERE account_id=?");
            if query.group_id.is_some() { sql.push_str(" AND group_id=?"); }
            if query.user_id.is_some() { sql.push_str(" AND user_id=?"); }
            if query.event.is_some() { sql.push_str(" AND event=?"); }
            if query.level.is_some() { sql.push_str(" AND level=?"); }
            if query.from.is_some() { sql.push_str(" AND created_at>=?"); }
            if query.to.is_some() { sql.push_str(" AND created_at<=?"); }
            if query.cursor.is_some() { sql.push_str(" AND id<?"); }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            let mut values: Vec<rusqlite::types::Value> = Vec::new();
            values.push(rusqlite::types::Value::from(query.account_id.clone()));
            if let Some(value) = query.group_id { values.push(value.into()); }
            if let Some(value) = query.user_id { values.push(value.into()); }
            if let Some(value) = &query.event { values.push(value.clone().into()); }
            if let Some(value) = &query.level { values.push(value.clone().into()); }
            if let Some(value) = value_to_string(&query.from) { values.push(value.into()); }
            if let Some(value) = value_to_string(&query.to) { values.push(value.into()); }
            if let Some(value) = &query.cursor { values.push(value.parse::<i64>().unwrap_or(i64::MAX).into()); }
            values.push((query.limit.unwrap_or(200).clamp(1, 1000) as i64).into());
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), |row| Ok(AuditEvent { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, user_id: row.get(3)?, actor: row.get(4)?, event: row.get(5)?, level: row.get(6)?, details: row.get(7)?, created_at: parse_time(row.get(8)?) }))?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("audit_query", error.to_string()))
    }
}

fn value_to_string(value: &Option<DateTime<Utc>>) -> Option<String> {
    value.as_ref().map(DateTime::to_rfc3339)
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
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
fn optional_time(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Account, Group};
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    fn database() -> Database {
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
    fn rules_and_actions_round_trip() {
        let database = database();
        let rule = ModerationRule {
            id: 0,
            account_id: "a".into(),
            group_id: 0,
            name: "关键词".into(),
            matcher: "contains".into(),
            pattern: "广告".into(),
            threshold: 0,
            count: 0,
            window_seconds: 0,
            cooldown_seconds: 0,
            priority: 10,
            mode: "observe".into(),
            enabled: true,
            semantic_threshold: 0.8,
            exempt_roles: vec!["admin".into()],
            exempt_user_ids: vec![],
            actions: vec![RuleAction {
                kind: "recall".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        };
        let id = database.save_rule(&rule).unwrap();
        let rules = database.list_rules("a", Some(1)).unwrap();
        assert_eq!(rules[0].id, id);
        assert_eq!(rules[0].actions[0].kind, "recall");
    }
    #[test]
    fn schedule_run_key_is_idempotent() {
        let database = database();
        let schedule = GroupSchedule {
            id: 0,
            account_id: "a".into(),
            name: "每日".into(),
            enabled: true,
            open_time: "08:00".into(),
            close_time: "22:00".into(),
            timezone: "local".into(),
            group_ids: vec![1],
        };
        let id = database.save_schedule(&schedule).unwrap();
        assert!(database
            .claim_schedule_run(id, "a", 1, "open", "same")
            .unwrap());
        assert!(!database
            .claim_schedule_run(id, "a", 1, "open", "same")
            .unwrap());
    }

    #[test]
    fn action_dedupe_updates_failure_and_preserves_single_row() {
        let database = database();
        let now = Utc::now();
        let mut action = ActionRecord {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            user_id: 2,
            message_id: Some(9),
            rule_id: Some(3),
            kind: "recall".into(),
            mode: "auto".into(),
            duration_seconds: 0,
            reason: "test".into(),
            success: false,
            error: "temporary".into(),
            dedupe_key: "message:9:rule:3:action:recall".into(),
            created_at: now,
        };
        let first = database.record_action(&action).unwrap();
        assert!(!database.action_succeeded("a", &action.dedupe_key).unwrap());
        action.success = true;
        action.error.clear();
        let second = database.record_action(&action).unwrap();
        assert_eq!(first, second);
        assert!(database.action_succeeded("a", &action.dedupe_key).unwrap());
    }
}
