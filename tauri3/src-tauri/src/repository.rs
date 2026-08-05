use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};

use crate::database::{Database, DatabaseExecutor};
use crate::error::{AppError, AppResult};
use crate::models::{
    ActionRecord, AuditEvent, DailySummary, GroupAiPermissions, GroupSchedule, KnowledgeBase,
    KnowledgeBinding, KnowledgeChunk, KnowledgeDocument, Message, ModerationRule, RuleAction,
    ScheduleRun, TaskItem, UniqueRun,
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
        self.with_connection(|connection| connection.execute("UPDATE groups SET enabled=?,ai_enabled=?,moderation_enabled=?,machine_rules_enabled=?,manual_takeover=?,updated_at=? WHERE account_id=? AND group_id=?", params![bool_i(enabled),bool_i(ai_enabled),bool_i(moderation_enabled),bool_i(moderation_enabled),bool_i(manual_takeover),Utc::now().to_rfc3339(),account_id,group_id]))
            .and_then(|changed| if changed == 1 { Ok(()) } else { Err(rusqlite::Error::QueryReturnedNoRows) })
            .map_err(|error| AppError::new("group_update", error.to_string()))
    }

    pub fn set_group_rule_features(
        &self,
        account_id: &str,
        group_id: i64,
        machine_enabled: bool,
        ai_enabled: bool,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("UPDATE groups SET moderation_enabled=?,machine_rules_enabled=?,ai_rules_enabled=?,updated_at=? WHERE account_id=? AND group_id=?", params![bool_i(machine_enabled),bool_i(machine_enabled),bool_i(ai_enabled),Utc::now().to_rfc3339(),account_id,group_id]))
            .and_then(|changed| if changed == 1 { Ok(()) } else { Err(rusqlite::Error::QueryReturnedNoRows) })
            .map_err(|error| AppError::new("group_rule_features", error.to_string()))
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
            let mut statement = connection.prepare("SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error,mentions_json,source_kind,flow FROM messages WHERE account_id=? AND (?2 IS NULL OR group_id=?2) ORDER BY received_at DESC,id DESC LIMIT ?3")?;
            let rows = statement.query_map(params![account_id, group_id, limit.clamp(1, 1000) as i64], message_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        }).map_err(|error| AppError::new("messages_read", error.to_string()))
    }

    pub(crate) fn query_messages(&self, query: &crate::MessageQuery) -> AppResult<Vec<Message>> {
        self.with_connection(|connection| {
            let mut sql = String::from("SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at,processing_state,attempts,next_attempt_at,last_error,mentions_json,source_kind,flow FROM messages WHERE account_id=?");
            let mut values = vec![rusqlite::types::Value::from(query.account_id.clone())];
            if !query.group_ids.is_empty() {
                sql.push_str(" AND group_id IN (");
                sql.push_str(&vec!["?"; query.group_ids.len()].join(","));
                sql.push(')');
                values.extend(query.group_ids.iter().copied().map(rusqlite::types::Value::from));
            }
            if let Some(keyword) = query.keyword.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                sql.push_str(" AND (LOWER(text) LIKE ? OR LOWER(sender_name) LIKE ? OR LOWER(server_message_id) LIKE ?)");
                let pattern = format!("%{}%", keyword.to_lowercase());
                values.extend([pattern.clone().into(), pattern.clone().into(), pattern.into()]);
            }
            if let Some(kind) = query.kind.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                sql.push_str(" AND kind=?");
                values.push(kind.to_string().into());
            }
            if let Some(state) = query.processing_state.as_deref().map(str::trim).filter(|value| !value.is_empty() && *value != "all") {
                sql.push_str(" AND processing_state=?");
                values.push(state.to_string().into());
            }
            if let Some(cursor) = query.cursor.as_deref().and_then(|value| value.parse::<i64>().ok()) {
                sql.push_str(" AND id<?");
                values.push(cursor.into());
            }
            sql.push_str(" ORDER BY id DESC LIMIT ?");
            values.push((query.limit.unwrap_or(50).clamp(1, 201) as i64).into());
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values), message_from_row)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("messages_query", error.to_string()))
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

    pub fn rule_cooldown_allows(
        &self,
        rule_id: i64,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
        cooldown_seconds: i64,
    ) -> AppResult<bool> {
        self.with_connection(|connection| {
            let last_executed: Option<String> = connection.query_row(
                "SELECT last_executed_at FROM rule_runtime_state WHERE rule_id=? AND account_id=? AND group_id=? AND user_id=?",
                params![rule_id, account_id, group_id, user_id],
                |row| row.get(0),
            ).optional()?.flatten();
            Ok(last_executed
                .and_then(|last| chrono::DateTime::parse_from_rfc3339(&last).ok())
                .map(|last| now.signed_duration_since(last.with_timezone(&Utc)).num_seconds() >= cooldown_seconds)
                .unwrap_or(true))
        }).map_err(|error| AppError::new("rule_runtime", error.to_string()))
    }

    pub fn mark_rule_executed(
        &self,
        rule_id: i64,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO rule_runtime_state(rule_id,account_id,group_id,user_id,window_started_at,window_count,last_executed_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(rule_id,account_id,group_id,user_id) DO UPDATE SET window_started_at=excluded.window_started_at,window_count=rule_runtime_state.window_count+1,last_executed_at=excluded.last_executed_at,updated_at=excluded.updated_at",
                params![rule_id, account_id, group_id, user_id, now.to_rfc3339(), 1_i64, now.to_rfc3339(), now.to_rfc3339()],
            )?;
            Ok(())
        }).map_err(|error| AppError::new("rule_runtime", error.to_string()))
    }

    pub fn list_rules(
        &self,
        account_id: &str,
        group_id: Option<i64>,
    ) -> AppResult<Vec<ModerationRule>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,account_id,group_id,rule_type,scope,priority_level,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json FROM rules WHERE account_id=? ORDER BY CASE priority_level WHEN 'high' THEN 3 WHEN 'low' THEN 1 ELSE 2 END DESC,id")?;
            let mut rules = statement.query_map(params![account_id], |row| {
                let roles: String = row.get(17)?; let users: String = row.get(18)?;
                Ok(ModerationRule { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, rule_type: row.get(3)?, scope: row.get(4)?, group_ids: Vec::new(), priority_level: row.get(5)?, whitelist_user_ids: Vec::new(), name: row.get(6)?, matcher: row.get(7)?, pattern: row.get(8)?, threshold: row.get(9)?, count: row.get(10)?, window_seconds: row.get(11)?, cooldown_seconds: row.get(12)?, priority: row.get(13)?, mode: row.get(14)?, enabled: row.get::<_, i64>(15)? != 0, semantic_threshold: row.get(16)?, exempt_roles: serde_json::from_str(&roles).unwrap_or_default(), exempt_user_ids: serde_json::from_str(&users).unwrap_or_default(), actions: Vec::new() })
            })?.collect::<Result<Vec<_>, _>>()?;
            for rule in &mut rules {
                let mut group_statement = connection.prepare("SELECT group_id FROM rule_groups WHERE rule_id=? ORDER BY group_id")?;
                rule.group_ids = group_statement.query_map(params![rule.id], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?;
                let mut whitelist_statement = connection.prepare("SELECT user_id FROM rule_whitelist_members WHERE rule_id=? ORDER BY user_id")?;
                rule.whitelist_user_ids = whitelist_statement.query_map(params![rule.id], |row| row.get(0))?.collect::<Result<Vec<_>, _>>()?;
                let mut action_statement = connection.prepare("SELECT kind,duration_seconds,message FROM rule_actions WHERE rule_id=? ORDER BY position,id")?;
                rule.actions = action_statement.query_map(params![rule.id], |row| Ok(RuleAction { kind: row.get(0)?, duration_seconds: row.get(1)?, message: row.get(2)? }))?.collect::<Result<Vec<_>, _>>()?;
            }
            if let Some(group_id) = group_id {
                rules.retain(|rule| rule.scope == "global" || rule.group_ids.contains(&group_id));
            }
            Ok(rules)
        }).map_err(|error| AppError::new("rules_read", error.to_string()))
    }

    pub fn save_rule(&self, rule: &ModerationRule) -> AppResult<i64> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let now = Utc::now().to_rfc3339();
            let legacy_group_id = if rule.scope == "selected" { rule.group_ids.first().copied().unwrap_or(0) } else { 0 };
            let priority = match rule.priority_level.as_str() { "high" => 300, "low" => 1, _ => 100 };
            let users = serde_json::to_string(&rule.whitelist_user_ids).unwrap_or_else(|_| "[]".into());
            let id = if rule.id > 0 {
                transaction.execute("UPDATE rules SET group_id=?,rule_type=?,scope=?,priority_level=?,name=?,matcher=?,pattern=?,threshold=?,count=?,window_seconds=?,cooldown_seconds=0,priority=?,mode=?,enabled=?,semantic_threshold=?,exempt_roles_json='[]',exempt_user_ids_json=?,updated_at=? WHERE id=? AND account_id=?", params![legacy_group_id,rule.rule_type,rule.scope,rule.priority_level,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,users,now,rule.id,rule.account_id])?;
                rule.id
            } else {
                transaction.execute("INSERT INTO rules(account_id,group_id,rule_type,scope,priority_level,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,0,?,?,?,?, '[]',?,?,?)", params![rule.account_id,legacy_group_id,rule.rule_type,rule.scope,rule.priority_level,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,users,now,now])?;
                transaction.last_insert_rowid()
            };
            transaction.execute("DELETE FROM rule_groups WHERE rule_id=?", params![id])?;
            if rule.scope == "selected" {
                for group_id in rule.group_ids.iter().copied().filter(|value| *value > 0) {
                    transaction.execute("INSERT OR IGNORE INTO rule_groups(rule_id,account_id,group_id) VALUES(?,?,?)", params![id,rule.account_id,group_id])?;
                }
            }
            transaction.execute("DELETE FROM rule_whitelist_members WHERE rule_id=?", params![id])?;
            for user_id in rule.whitelist_user_ids.iter().copied().filter(|value| *value > 0) {
                transaction.execute("INSERT OR IGNORE INTO rule_whitelist_members(rule_id,user_id) VALUES(?,?)", params![id,user_id])?;
            }
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
                let legacy_group_id = if rule.scope == "selected" { rule.group_ids.first().copied().unwrap_or(0) } else { 0 };
                let priority = match rule.priority_level.as_str() { "high" => 300, "low" => 1, _ => 100 };
                let users = serde_json::to_string(&rule.whitelist_user_ids).unwrap_or_else(|_| "[]".into());
                transaction.execute("INSERT INTO rules(account_id,group_id,rule_type,scope,priority_level,name,matcher,pattern,threshold,count,window_seconds,cooldown_seconds,priority,mode,enabled,semantic_threshold,exempt_roles_json,exempt_user_ids_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,0,?,?,?,?, '[]',?,?,?)", params![account_id,legacy_group_id,rule.rule_type,rule.scope,rule.priority_level,rule.name,rule.matcher,rule.pattern,rule.threshold,rule.count,rule.window_seconds,priority,rule.mode,bool_i(rule.enabled),rule.semantic_threshold,users,now,now])?;
                let rule_id = transaction.last_insert_rowid();
                if rule.scope == "selected" { for group_id in rule.group_ids.iter().copied().filter(|value| *value > 0) { transaction.execute("INSERT OR IGNORE INTO rule_groups(rule_id,account_id,group_id) VALUES(?,?,?)", params![rule_id,account_id,group_id])?; } }
                for user_id in rule.whitelist_user_ids.iter().copied().filter(|value| *value > 0) { transaction.execute("INSERT OR IGNORE INTO rule_whitelist_members(rule_id,user_id) VALUES(?,?)", params![rule_id,user_id])?; }
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

    #[allow(clippy::too_many_arguments)]
    pub fn record_rule_evaluation(
        &self,
        account_id: &str,
        group_id: i64,
        user_id: i64,
        message_id: i64,
        rule_id: i64,
        rule_type: &str,
        matched: bool,
        confidence: Option<f64>,
        mode: &str,
        decision: &str,
        reason: &str,
        elapsed_ms: i64,
    ) -> AppResult<()> {
        self.with_connection(|connection| connection.execute("INSERT INTO rule_evaluations(account_id,group_id,user_id,message_id,rule_id,rule_type,matched,confidence,mode,decision,reason,elapsed_ms,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,message_id,rule_id) DO UPDATE SET matched=excluded.matched,confidence=excluded.confidence,mode=excluded.mode,decision=excluded.decision,reason=excluded.reason,elapsed_ms=excluded.elapsed_ms", params![account_id,group_id,user_id,message_id,rule_id,rule_type,bool_i(matched),confidence,mode,decision,reason,elapsed_ms,Utc::now().to_rfc3339()]))
            .map(|_| ()).map_err(|error| AppError::new("rule_evaluation_write", error.to_string()))
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
                "INSERT INTO knowledge_documents(base_id,title,kind,content,source,content_hash,enabled,created_at,updated_at) SELECT ?,title,kind,content,source,content_hash,enabled,created_at,updated_at FROM knowledge_documents WHERE base_id=?",
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
                connection.execute("UPDATE knowledge_documents SET title=?,kind=?,content=?,source=?,content_hash=?,enabled=?,updated_at=? WHERE id=? AND base_id=?", params![document.title,document.kind,document.content,document.source,hash,bool_i(document.enabled),now,document.id,document.base_id])?;
                Ok(document.id)
            } else {
                connection.execute("INSERT INTO knowledge_documents(base_id,title,kind,content,source,content_hash,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(base_id,content_hash) DO UPDATE SET title=excluded.title,kind=excluded.kind,source=excluded.source,enabled=excluded.enabled,updated_at=excluded.updated_at", params![document.base_id,document.title,document.kind,document.content,document.source,hash,bool_i(document.enabled),now,now])?;
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
                "SELECT d.id,d.base_id,b.name,d.title,d.kind,d.content,d.source,d.content_hash,d.enabled FROM knowledge_documents d JOIN knowledge_bases b ON b.id=d.base_id WHERE d.base_id=? ORDER BY d.title,d.id",
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
                    enabled: row.get::<_, i64>(8)? != 0,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("knowledge_documents_read", error.to_string()))
    }

    pub fn replace_knowledge_chunks(
        &self,
        document_id: i64,
        chunks: &[KnowledgeChunk],
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "DELETE FROM knowledge_chunks WHERE document_id=?",
                params![document_id],
            )?;
            let now = Utc::now().to_rfc3339();
            for (position, chunk) in chunks.iter().enumerate() {
                let chunk_index = if chunk.chunk_index >= 0 {
                    chunk.chunk_index
                } else {
                    position as i64
                };
                transaction.execute(
                    "INSERT INTO knowledge_chunks(document_id,chunk_index,content,content_hash,token_count,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)",
                    params![document_id,chunk_index,chunk.content,chunk.content_hash,chunk.token_count,bool_i(chunk.enabled),now,now],
                )?;
            }
            transaction.commit()
        })
        .map_err(|error| AppError::new("knowledge_chunks_write", error.to_string()))
    }

    pub fn list_knowledge_chunks(
        &self,
        document_id: i64,
        enabled_only: bool,
    ) -> AppResult<Vec<KnowledgeChunk>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,document_id,chunk_index,content,content_hash,token_count,enabled FROM knowledge_chunks WHERE document_id=? AND (?2=0 OR enabled=1) ORDER BY chunk_index,id",
            )?;
            let rows = statement.query_map(
                params![document_id, bool_i(enabled_only)],
                |row| {
                    Ok(KnowledgeChunk {
                        id: row.get(0)?,
                        document_id: row.get(1)?,
                        chunk_index: row.get(2)?,
                        content: row.get(3)?,
                        content_hash: row.get(4)?,
                        token_count: row.get(5)?,
                        enabled: row.get::<_, i64>(6)? != 0,
                    })
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("knowledge_chunks_read", error.to_string()))
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
            if task.id > 0 { let reminder_at=task.reminder_at.map(|value| value.to_rfc3339()); connection.execute("UPDATE tasks SET title=?,description=?,status=?,assignee_id=?,created_by=?,due_at=?,reminder_state=CASE WHEN reminder_at IS NOT ? THEN 'pending' ELSE reminder_state END,reminder_attempts=CASE WHEN reminder_at IS NOT ? THEN 0 ELSE reminder_attempts END,reminder_next_attempt_at=CASE WHEN reminder_at IS NOT ? THEN NULL ELSE reminder_next_attempt_at END,reminder_claimed_at=CASE WHEN reminder_at IS NOT ? THEN NULL ELSE reminder_claimed_at END,reminder_last_error=CASE WHEN reminder_at IS NOT ? THEN '' ELSE reminder_last_error END,reminder_sent_at=CASE WHEN reminder_at IS NOT ? THEN NULL ELSE reminder_sent_at END,reminder_at=?,updated_at=? WHERE id=? AND account_id=?", params![task.title,task.description,task.status,task.assignee_id,task.created_by,task.due_at.map(|value| value.to_rfc3339()),reminder_at,reminder_at,reminder_at,reminder_at,reminder_at,reminder_at,reminder_at,now,task.id,task.account_id])?; Ok(task.id) }
            else { connection.execute("INSERT INTO tasks(account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)", params![task.account_id,task.group_id,task.title,task.description,task.status,task.assignee_id,task.created_by,task.due_at.map(|value| value.to_rfc3339()),task.reminder_at.map(|value| value.to_rfc3339()),now,now])?; Ok(connection.last_insert_rowid()) }
        }).map_err(|error| AppError::new("task_write", error.to_string()))
    }

    pub fn save_task_once(&self, task: &TaskItem, source_key: &str) -> AppResult<i64> {
        if source_key.trim().is_empty() {
            return self.save_task(task);
        }
        self.with_connection(|connection| {
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT OR IGNORE INTO tasks(account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,source_key,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                params![task.account_id,task.group_id,task.title,task.description,task.status,task.assignee_id,task.created_by,task.due_at.map(|value|value.to_rfc3339()),task.reminder_at.map(|value|value.to_rfc3339()),source_key,now,now],
            )?;
            connection.query_row(
                "SELECT id FROM tasks WHERE account_id=? AND source_key=?",
                params![task.account_id, source_key],
                |row| row.get(0),
            )
        })
        .map_err(|error| AppError::new("task_write", error.to_string()))
    }

    pub fn claim_due_task_reminders(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> AppResult<Vec<TaskItem>> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let rows = {
                let mut statement = tx.prepare("SELECT id,account_id,group_id,title,description,status,assignee_id,created_by,due_at,reminder_at,reminder_sent_at,created_at,updated_at FROM tasks WHERE reminder_at IS NOT NULL AND reminder_at<=? AND reminder_sent_at IS NULL AND reminder_state IN ('pending','retry') AND (reminder_next_attempt_at IS NULL OR reminder_next_attempt_at<=?) AND status!='done' ORDER BY reminder_at,id LIMIT ?")?;
                let result = statement.query_map(params![now.to_rfc3339(), now.to_rfc3339(), limit.clamp(1, 100) as i64], |row| Ok(TaskItem { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, title: row.get(3)?, description: row.get(4)?, status: row.get(5)?, assignee_id: row.get(6)?, created_by: row.get(7)?, due_at: optional_time(row.get(8)?), reminder_at: optional_time(row.get(9)?), reminder_sent_at: optional_time(row.get(10)?), created_at: parse_time(row.get(11)?), updated_at: parse_time(row.get(12)?) }))?.collect::<Result<Vec<_>, _>>()?;
                result
            };
            for task in &rows { tx.execute("UPDATE tasks SET reminder_state='processing',reminder_attempts=reminder_attempts+1,reminder_claimed_at=?,reminder_next_attempt_at=NULL WHERE id=? AND reminder_sent_at IS NULL AND reminder_state IN ('pending','retry')", params![now.to_rfc3339(), task.id])?; }
            tx.commit()?;
            Ok(rows)
        }).map_err(|error| AppError::new("task_reminder_claim", error.to_string()))
    }

    pub fn finish_task_reminder(&self, task_id: i64, success: bool, error: &str) -> AppResult<()> {
        let now = Utc::now();
        self.with_connection(|connection| {
            if success {
                connection.execute(
                    "UPDATE tasks SET reminder_sent_at=?,reminder_state='sent',reminder_claimed_at=NULL,reminder_next_attempt_at=NULL,reminder_last_error='',updated_at=? WHERE id=? AND reminder_state='processing'",
                    params![now.to_rfc3339(), now.to_rfc3339(), task_id],
                )?;
            } else {
                let attempts: i64 = connection
                    .query_row(
                        "SELECT reminder_attempts FROM tasks WHERE id=?",
                        params![task_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or(1);
                let state = if attempts >= 5 { "failed" } else { "retry" };
                let next_retry = (attempts < 5).then(|| {
                    (now + reminder_retry_delay(attempts)).to_rfc3339()
                });
                connection.execute(
                    "UPDATE tasks SET reminder_state=?,reminder_claimed_at=NULL,reminder_next_attempt_at=?,reminder_last_error=?,updated_at=? WHERE id=? AND reminder_state='processing'",
                    params![state,next_retry,error,now.to_rfc3339(),task_id],
                )?;
            }
            Ok(())
        })
        .map_err(|error| AppError::new("task_reminder_finish", error.to_string()))
    }

    pub fn mark_task_reminder_unknown(&self, task_id: i64, error: &str) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE tasks SET reminder_state='unknown',reminder_claimed_at=NULL,reminder_next_attempt_at=NULL,reminder_last_error=?,updated_at=? WHERE id=? AND reminder_state='processing'",
                params![error, Utc::now().to_rfc3339(), task_id],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("task_reminder_unknown", error.to_string()))
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

    pub fn claim_ai_run(&self, account_id: &str, group_id: i64, run_key: &str) -> AppResult<bool> {
        self.with_connection(|connection| {
            claim_unique_run(connection, "ai_runs", account_id, group_id, run_key, false)
        })
        .map_err(|error| AppError::new("ai_run_claim", error.to_string()))
    }

    pub fn finish_ai_run(
        &self,
        account_id: &str,
        run_key: &str,
        success: bool,
        error: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            finish_unique_run(
                connection, "ai_runs", account_id, None, run_key, success, error,
            )
        })
        .map_err(|error| AppError::new("ai_run_finish", error.to_string()))
    }

    pub fn fail_ai_run_terminal(
        &self,
        account_id: &str,
        run_key: &str,
        error: &str,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE ai_runs SET state='failed',completed_at=?,next_retry_at=NULL,last_error=?,updated_at=? WHERE account_id=? AND run_key=? AND state='processing'",
                params![now, error, now, account_id, run_key],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("ai_run_finish", error.to_string()))
    }

    pub fn ai_run(&self, account_id: &str, run_key: &str) -> AppResult<Option<UniqueRun>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,account_id,group_id,run_key,state,attempts,next_retry_at,last_error,created_at,updated_at,completed_at FROM ai_runs WHERE account_id=? AND run_key=?",
                    params![account_id, run_key],
                    unique_run_from_row,
                )
                .optional()
        })
        .map_err(|error| AppError::new("ai_run_read", error.to_string()))
    }

    pub fn claim_summary_run(
        &self,
        account_id: &str,
        group_id: i64,
        run_key: &str,
    ) -> AppResult<bool> {
        self.with_connection(|connection| {
            claim_unique_run(
                connection,
                "summary_runs",
                account_id,
                group_id,
                run_key,
                true,
            )
        })
        .map_err(|error| AppError::new("summary_run_claim", error.to_string()))
    }

    pub fn finish_summary_run(
        &self,
        account_id: &str,
        group_id: i64,
        run_key: &str,
        success: bool,
        error: &str,
    ) -> AppResult<()> {
        self.with_connection(|connection| {
            finish_unique_run(
                connection,
                "summary_runs",
                account_id,
                Some(group_id),
                run_key,
                success,
                error,
            )
        })
        .map_err(|error| AppError::new("summary_run_finish", error.to_string()))
    }

    pub fn summary_run(
        &self,
        account_id: &str,
        group_id: i64,
        run_key: &str,
    ) -> AppResult<Option<UniqueRun>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,account_id,group_id,run_key,state,attempts,next_retry_at,last_error,created_at,updated_at,completed_at FROM summary_runs WHERE account_id=? AND group_id=? AND run_key=?",
                    params![account_id, group_id, run_key],
                    unique_run_from_row,
                )
                .optional()
        })
        .map_err(|error| AppError::new("summary_run_read", error.to_string()))
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

    pub fn mark_schedule_run_unknown(&self, run_key: &str, error: &str) -> AppResult<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE schedule_runs SET success=0,error=?,attempts=MAX(attempts,5),next_retry_at=NULL WHERE run_key=?",
                params![error, run_key],
            )?;
            Ok(())
        })
        .map_err(|error| AppError::new("schedule_run_unknown", error.to_string()))
    }

    pub fn record_action(&self, action: &ActionRecord) -> AppResult<i64> {
        self.with_connection(|connection| {
            connection.execute("INSERT INTO actions(account_id,group_id,user_id,message_id,rule_id,kind,mode,duration_seconds,reason,success,error,receipt_json,dedupe_key,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,dedupe_key) WHERE dedupe_key<>'' DO UPDATE SET success=excluded.success,error=excluded.error,receipt_json=excluded.receipt_json,reason=excluded.reason,created_at=excluded.created_at", params![action.account_id,action.group_id,action.user_id,action.message_id,action.rule_id,action.kind,action.mode,action.duration_seconds,action.reason,bool_i(action.success),action.error,action.receipt_json,action.dedupe_key,action.created_at.to_rfc3339()])?;
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

    /// Commits the externally observed effect result and its operator-visible
    /// records as one unit. A crash can therefore leave the claimed effect in
    /// `processing` (recovered as `unknown` on restart), but can never expose a
    /// terminal outbox row without its matching action/audit records.
    pub fn archive_effect_dispatch(
        &self,
        outbox_id: i64,
        status: &str,
        error: &str,
        receipt_json: &str,
        action: Option<&ActionRecord>,
        audit: &AuditEvent,
    ) -> AppResult<bool> {
        let now = Utc::now();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let changed = match status {
                "succeeded" => transaction.execute(
                    "UPDATE effect_outbox SET state='succeeded',completed_at=?,claimed_at=NULL,next_attempt_at=NULL,last_error='',receipt_json=? WHERE id=? AND state='processing'",
                    params![now.to_rfc3339(), receipt_json, outbox_id],
                )?,
                "unknown" => transaction.execute(
                    "UPDATE effect_outbox SET state='unknown',claimed_at=NULL,next_attempt_at=NULL,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
                    params![error, receipt_json, outbox_id],
                )?,
                "failed-terminal" => transaction.execute(
                    "UPDATE effect_outbox SET state='failed',completed_at=?,claimed_at=NULL,next_attempt_at=NULL,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
                    params![now.to_rfc3339(), error, receipt_json, outbox_id],
                )?,
                _ => {
                    let attempts: i64 = transaction
                        .query_row(
                            "SELECT attempts FROM effect_outbox WHERE id=?",
                            params![outbox_id],
                            |row| row.get(0),
                        )
                        .optional()?
                        .unwrap_or(1);
                    let state = if attempts >= 5 { "failed" } else { "retry" };
                    let next_attempt_at = (attempts < 5).then(|| {
                        (now + chrono::Duration::seconds(match attempts {
                            0 | 1 => 5,
                            2 => 30,
                            3 => 120,
                            4 => 600,
                            _ => 1_800,
                        }))
                        .to_rfc3339()
                    });
                    transaction.execute(
                        "UPDATE effect_outbox SET state=?,claimed_at=NULL,next_attempt_at=?,last_error=?,receipt_json=? WHERE id=? AND state='processing'",
                        params![state, next_attempt_at, error, receipt_json, outbox_id],
                    )?
                }
            };
            if changed == 0 {
                transaction.commit()?;
                return Ok(false);
            }
            if let Some(action) = action {
                transaction.execute(
                    "INSERT INTO actions(account_id,group_id,user_id,message_id,rule_id,kind,mode,duration_seconds,reason,success,error,receipt_json,dedupe_key,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,dedupe_key) WHERE dedupe_key<>'' DO UPDATE SET success=excluded.success,error=excluded.error,receipt_json=excluded.receipt_json,reason=excluded.reason,created_at=excluded.created_at",
                    params![action.account_id,action.group_id,action.user_id,action.message_id,action.rule_id,action.kind,action.mode,action.duration_seconds,action.reason,bool_i(action.success),action.error,action.receipt_json,action.dedupe_key,action.created_at.to_rfc3339()],
                )?;
            }
            transaction.execute(
                "INSERT INTO audit_events(account_id,group_id,user_id,actor,event,level,details,created_at) VALUES(?,?,?,?,?,?,?,?)",
                params![audit.account_id,audit.group_id,audit.user_id,audit.actor,audit.event,audit.level,audit.details,audit.created_at.to_rfc3339()],
            )?;
            transaction.commit()?;
            Ok(true)
        })
        .map_err(|error| AppError::new("effect_dispatch_archive", error.to_string()))
    }

    pub fn list_audit(&self, account_id: &str, limit: usize) -> AppResult<Vec<AuditEvent>> {
        self.with_connection(|connection| { let mut statement = connection.prepare("SELECT id,account_id,group_id,user_id,actor,event,level,details,created_at FROM audit_events WHERE account_id=? ORDER BY created_at DESC,id DESC LIMIT ?")?; let rows = statement.query_map(params![account_id,limit.clamp(1,1000) as i64], |row| Ok(AuditEvent { id: row.get(0)?, account_id: row.get(1)?, group_id: row.get(2)?, user_id: row.get(3)?, actor: row.get(4)?, event: row.get(5)?, level: row.get(6)?, details: row.get(7)?, created_at: parse_time(row.get(8)?) }))?; rows.collect::<Result<Vec<_>,_>>() }).map_err(|error| AppError::new("audit_read", error.to_string()))
    }

    /// Support bundles are produced locally and need a small, cross-account
    /// audit window even when the currently logged-in account is unavailable.
    /// The caller aliases every identity before the data leaves this process.
    pub fn list_support_audit(&self, limit: usize) -> AppResult<Vec<AuditEvent>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,account_id,group_id,user_id,actor,event,level,details,created_at \
                 FROM audit_events ORDER BY id DESC LIMIT ?",
            )?;
            let rows = statement.query_map([limit.clamp(1, 500) as i64], |row| {
                Ok(AuditEvent {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    group_id: row.get(2)?,
                    user_id: row.get(3)?,
                    actor: row.get(4)?,
                    event: row.get(5)?,
                    level: row.get(6)?,
                    details: row.get(7)?,
                    created_at: parse_time(row.get(8)?),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| AppError::new("audit_read", error.to_string()))
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

impl DatabaseExecutor {
    pub(crate) async fn query_messages(
        &self,
        query: crate::MessageQuery,
    ) -> AppResult<Vec<Message>> {
        self.execute(move |database| database.query_messages(&query))
            .await
    }

    pub async fn archive_effect_dispatch(
        &self,
        outbox_id: i64,
        status: String,
        error: String,
        receipt_json: String,
        action: Option<ActionRecord>,
        audit: AuditEvent,
    ) -> AppResult<bool> {
        self.execute(move |database| {
            database.archive_effect_dispatch(
                outbox_id,
                &status,
                &error,
                &receipt_json,
                action.as_ref(),
                &audit,
            )
        })
        .await
    }

    pub async fn set_setting(&self, key: String, value: String, sensitive: bool) -> AppResult<()> {
        self.execute(move |database| database.set_setting(&key, &value, sensitive))
            .await
    }

    pub async fn set_group_features(
        &self,
        account_id: String,
        group_id: i64,
        enabled: bool,
        ai_enabled: bool,
        moderation_enabled: bool,
        manual_takeover: bool,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.set_group_features(
                &account_id,
                group_id,
                enabled,
                ai_enabled,
                moderation_enabled,
                manual_takeover,
            )
        })
        .await
    }

    pub async fn set_group_rule_features(
        &self,
        account_id: String,
        group_id: i64,
        machine_enabled: bool,
        ai_enabled: bool,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.set_group_rule_features(&account_id, group_id, machine_enabled, ai_enabled)
        })
        .await
    }

    pub async fn set_group_welcome(
        &self,
        account_id: String,
        group_id: i64,
        welcome_message: String,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.set_group_welcome(&account_id, group_id, &welcome_message)
        })
        .await
    }

    pub async fn save_group_ai_permissions(&self, value: GroupAiPermissions) -> AppResult<()> {
        self.execute(move |database| database.save_group_ai_permissions(&value))
            .await
    }

    pub async fn save_rule(&self, rule: ModerationRule) -> AppResult<i64> {
        self.execute(move |database| database.save_rule(&rule))
            .await
    }

    pub async fn delete_rule(&self, account_id: String, rule_id: i64) -> AppResult<()> {
        self.execute(move |database| database.delete_rule(&account_id, rule_id))
            .await
    }

    pub async fn import_rules(
        &self,
        account_id: String,
        rules: Vec<ModerationRule>,
    ) -> AppResult<usize> {
        self.execute(move |database| database.import_rules(&account_id, &rules))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_rule_evaluation(
        &self,
        account_id: String,
        group_id: i64,
        user_id: i64,
        message_id: i64,
        rule_id: i64,
        rule_type: String,
        matched: bool,
        confidence: Option<f64>,
        mode: String,
        decision: String,
        reason: String,
        elapsed_ms: i64,
    ) -> AppResult<()> {
        self.execute(move |database| {
            database.record_rule_evaluation(
                &account_id,
                group_id,
                user_id,
                message_id,
                rule_id,
                &rule_type,
                matched,
                confidence,
                &mode,
                &decision,
                &reason,
                elapsed_ms,
            )
        })
        .await
    }

    pub async fn list_knowledge_bases(&self, account_id: String) -> AppResult<Vec<KnowledgeBase>> {
        self.execute(move |database| database.list_knowledge_bases(&account_id))
            .await
    }

    pub async fn create_knowledge_base(&self, base: KnowledgeBase) -> AppResult<i64> {
        self.execute(move |database| database.create_knowledge_base(&base))
            .await
    }

    pub async fn update_knowledge_base(&self, base: KnowledgeBase) -> AppResult<()> {
        self.execute(move |database| database.update_knowledge_base(&base))
            .await
    }

    pub async fn clone_knowledge_base(
        &self,
        account_id: String,
        base_id: i64,
        name: String,
    ) -> AppResult<i64> {
        self.execute(move |database| database.clone_knowledge_base(&account_id, base_id, &name))
            .await
    }

    pub async fn delete_knowledge_base(&self, account_id: String, base_id: i64) -> AppResult<()> {
        self.execute(move |database| database.delete_knowledge_base(&account_id, base_id))
            .await
    }

    pub async fn knowledge_base_account(&self, base_id: i64) -> AppResult<String> {
        self.execute(move |database| database.knowledge_base_account(base_id))
            .await
    }

    pub async fn list_knowledge_documents(
        &self,
        base_id: i64,
    ) -> AppResult<Vec<KnowledgeDocument>> {
        self.execute(move |database| database.list_knowledge_documents(base_id))
            .await
    }

    pub async fn upsert_knowledge_document(&self, document: KnowledgeDocument) -> AppResult<i64> {
        self.execute(move |database| database.upsert_knowledge_document(&document))
            .await
    }

    pub async fn delete_knowledge_document(&self, base_id: i64, document_id: i64) -> AppResult<()> {
        self.execute(move |database| database.delete_knowledge_document(base_id, document_id))
            .await
    }

    pub async fn replace_knowledge_chunks(
        &self,
        document_id: i64,
        chunks: Vec<KnowledgeChunk>,
    ) -> AppResult<()> {
        self.execute(move |database| database.replace_knowledge_chunks(document_id, &chunks))
            .await
    }

    pub async fn bind_knowledge_base(
        &self,
        account_id: String,
        base_id: i64,
        group_ids: Vec<i64>,
    ) -> AppResult<()> {
        self.execute(move |database| database.bind_knowledge_base(base_id, &account_id, &group_ids))
            .await
    }

    pub async fn list_knowledge_bindings(
        &self,
        account_id: String,
        base_id: Option<i64>,
    ) -> AppResult<Vec<KnowledgeBinding>> {
        self.execute(move |database| database.list_knowledge_bindings(&account_id, base_id))
            .await
    }

    pub async fn list_tasks(
        &self,
        account_id: String,
        group_id: Option<i64>,
    ) -> AppResult<Vec<TaskItem>> {
        self.execute(move |database| database.list_tasks(&account_id, group_id))
            .await
    }

    pub async fn save_task(&self, task: TaskItem) -> AppResult<i64> {
        self.execute(move |database| database.save_task(&task))
            .await
    }

    pub async fn delete_task(&self, account_id: String, task_id: i64) -> AppResult<()> {
        self.execute(move |database| database.delete_task(&account_id, task_id))
            .await
    }

    pub async fn save_schedule(&self, schedule: GroupSchedule) -> AppResult<i64> {
        self.execute(move |database| database.save_schedule(&schedule))
            .await
    }

    pub async fn delete_schedule(&self, account_id: String, schedule_id: i64) -> AppResult<()> {
        self.execute(move |database| database.delete_schedule(&account_id, schedule_id))
            .await
    }

    pub async fn list_schedule_runs(
        &self,
        account_id: String,
        schedule_id: Option<i64>,
        limit: usize,
    ) -> AppResult<Vec<ScheduleRun>> {
        self.execute(move |database| database.list_schedule_runs(&account_id, schedule_id, limit))
            .await
    }

    pub async fn list_audit(&self, account_id: String, limit: usize) -> AppResult<Vec<AuditEvent>> {
        self.execute(move |database| database.list_audit(&account_id, limit))
            .await
    }

    pub async fn list_support_audit(&self, limit: usize) -> AppResult<Vec<AuditEvent>> {
        self.execute(move |database| database.list_support_audit(limit))
            .await
    }

    pub(crate) async fn query_audit(&self, query: crate::AuditQuery) -> AppResult<Vec<AuditEvent>> {
        self.execute(move |database| database.query_audit(&query))
            .await
    }
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
        sent_at: parse_time(row.get::<_, String>(9)?),
        received_at: parse_time(row.get::<_, String>(10)?),
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

fn value_to_string(value: &Option<DateTime<Utc>>) -> Option<String> {
    value.as_ref().map(DateTime::to_rfc3339)
}

fn claim_unique_run(
    connection: &mut Connection,
    table: &str,
    account_id: &str,
    group_id: i64,
    run_key: &str,
    group_scoped: bool,
) -> rusqlite::Result<bool> {
    let transaction = connection.transaction()?;
    let now = Utc::now().to_rfc3339();
    let changed = transaction.execute(
        &format!("INSERT OR IGNORE INTO {table}(account_id,group_id,run_key,state,attempts,next_retry_at,last_error,created_at,updated_at,completed_at) VALUES(?,?,?,'processing',1,NULL,'',?,?,NULL)"),
        params![account_id, group_id, run_key, now, now],
    )?;
    if changed == 1 {
        transaction.commit()?;
        return Ok(true);
    }
    let existing: Option<(String, i64, Option<String>)> = if group_scoped {
        transaction
            .query_row(
                &format!("SELECT state,attempts,next_retry_at FROM {table} WHERE account_id=? AND group_id=? AND run_key=?"),
                params![account_id, group_id, run_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
    } else {
        transaction
            .query_row(
                &format!("SELECT state,attempts,next_retry_at FROM {table} WHERE account_id=? AND run_key=?"),
                params![account_id, run_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
    };
    let claim = existing
        .map(|(state, attempts, next_retry)| {
            state == "retry"
                && attempts < 5
                && next_retry
                    .as_deref()
                    .map(|value| value <= now.as_str())
                    .unwrap_or(true)
        })
        .unwrap_or(false);
    if claim {
        if group_scoped {
            transaction.execute(
                &format!("UPDATE {table} SET state='processing',attempts=attempts+1,next_retry_at=NULL,last_error='',updated_at=? WHERE account_id=? AND group_id=? AND run_key=? AND state='retry'"),
                params![now, account_id, group_id, run_key],
            )?;
        } else {
            transaction.execute(
                &format!("UPDATE {table} SET state='processing',attempts=attempts+1,next_retry_at=NULL,last_error='',updated_at=? WHERE account_id=? AND run_key=? AND state='retry'"),
                params![now, account_id, run_key],
            )?;
        }
    }
    transaction.commit()?;
    Ok(claim)
}

fn finish_unique_run(
    connection: &mut Connection,
    table: &str,
    account_id: &str,
    group_id: Option<i64>,
    run_key: &str,
    success: bool,
    error: &str,
) -> rusqlite::Result<()> {
    let now = Utc::now();
    let attempts: Option<i64> = if let Some(group_id) = group_id {
        connection
            .query_row(
                &format!(
                    "SELECT attempts FROM {table} WHERE account_id=? AND group_id=? AND run_key=?"
                ),
                params![account_id, group_id, run_key],
                |row| row.get(0),
            )
            .optional()?
    } else {
        connection
            .query_row(
                &format!("SELECT attempts FROM {table} WHERE account_id=? AND run_key=?"),
                params![account_id, run_key],
                |row| row.get(0),
            )
            .optional()?
    };
    let Some(attempts) = attempts else {
        return Ok(());
    };
    let state = if success {
        "succeeded"
    } else if attempts >= 5 {
        "failed"
    } else {
        "retry"
    };
    let next_retry_at =
        (!success && attempts < 5).then(|| (now + reminder_retry_delay(attempts)).to_rfc3339());
    let completed_at = success.then(|| now.to_rfc3339());
    let last_error = if success { "" } else { error };
    if let Some(group_id) = group_id {
        connection.execute(
            &format!("UPDATE {table} SET state=?,next_retry_at=?,last_error=?,updated_at=?,completed_at=? WHERE account_id=? AND group_id=? AND run_key=? AND state='processing'"),
            params![state,next_retry_at,last_error,now.to_rfc3339(),completed_at,account_id,group_id,run_key],
        )?;
    } else {
        connection.execute(
            &format!("UPDATE {table} SET state=?,next_retry_at=?,last_error=?,updated_at=?,completed_at=? WHERE account_id=? AND run_key=? AND state='processing'"),
            params![state,next_retry_at,last_error,now.to_rfc3339(),completed_at,account_id,run_key],
        )?;
    }
    Ok(())
}

fn unique_run_from_row(row: &Row<'_>) -> rusqlite::Result<UniqueRun> {
    Ok(UniqueRun {
        id: row.get(0)?,
        account_id: row.get(1)?,
        group_id: row.get(2)?,
        run_key: row.get(3)?,
        state: row.get(4)?,
        attempts: row.get(5)?,
        next_retry_at: optional_time(row.get(6)?),
        last_error: row.get(7)?,
        created_at: parse_time(row.get(8)?),
        updated_at: parse_time(row.get(9)?),
        completed_at: optional_time(row.get(10)?),
    })
}

fn reminder_retry_delay(attempts: i64) -> chrono::Duration {
    chrono::Duration::seconds(match attempts {
        0 | 1 => 5,
        2 => 30,
        3 => 120,
        4 => 600,
        _ => 1800,
    })
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
    use crate::models::{Account, EffectOutboxRequest, Group};
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
    fn message_query_filters_and_paginates_in_sqlite() {
        let database = database();
        let now = Utc::now();
        let first = Message {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            server_message_id: "message-1".into(),
            sequence: 1,
            user_id: 7,
            sender_name: "广州校长".into(),
            kind: "text".into(),
            text: "第一条业务消息".into(),
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
        };
        let mut second = first.clone();
        second.server_message_id = "message-2".into();
        second.sequence = 2;
        second.text = "第二条业务消息".into();
        let first_id = database.insert_message(&first).unwrap().id;
        let second_id = database.insert_message(&second).unwrap().id;

        let page = database
            .query_messages(&crate::MessageQuery {
                account_id: "a".into(),
                group_ids: vec![1],
                keyword: Some("业务".into()),
                kind: Some("text".into()),
                processing_state: Some("pending".into()),
                cursor: None,
                limit: Some(1),
            })
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, second_id);

        let previous = database
            .query_messages(&crate::MessageQuery {
                account_id: "a".into(),
                group_ids: vec![1],
                keyword: Some("第一条".into()),
                kind: Some("text".into()),
                processing_state: Some("pending".into()),
                cursor: Some(second_id.to_string()),
                limit: Some(10),
            })
            .unwrap();
        assert_eq!(previous.len(), 1);
        assert_eq!(previous[0].id, first_id);
    }

    #[test]
    fn effect_dispatch_archive_is_atomic_across_outbox_action_and_audit() {
        let database = database();
        let effect = database
            .enqueue_effect(&EffectOutboxRequest {
                account_id: "a".into(),
                group_id: 1,
                effect_type: "send_text".into(),
                payload_json: r#"{"text":"hello"}"#.into(),
                dedupe_key: "atomic-effect".into(),
            })
            .unwrap();
        let claimed = database.claim_effect_outbox(Some("a"), 1).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, effect.id);
        let now = Utc::now();
        let action = ActionRecord {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            user_id: 2,
            message_id: Some(3),
            rule_id: Some(4),
            kind: "reply".into(),
            mode: "automatic".into(),
            duration_seconds: 0,
            reason: "atomic".into(),
            success: true,
            error: String::new(),
            receipt_json: r#"{"status":"succeeded"}"#.into(),
            dedupe_key: "atomic-effect".into(),
            created_at: now,
        };
        let audit = AuditEvent {
            id: 0,
            account_id: "a".into(),
            group_id: 1,
            user_id: 2,
            actor: "DH BOT".into(),
            event: "effect_dispatched".into(),
            level: "info".into(),
            details: r#"{"status":"succeeded"}"#.into(),
            created_at: now,
        };
        database
            .with_connection(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_effect_audit
                     BEFORE INSERT ON audit_events
                     WHEN NEW.event='effect_dispatched'
                     BEGIN SELECT RAISE(ABORT, 'injected archive failure'); END;",
                )
            })
            .unwrap();
        let error = database
            .archive_effect_dispatch(
                effect.id,
                "succeeded",
                "",
                &action.receipt_json,
                Some(&action),
                &audit,
            )
            .unwrap_err();
        assert_eq!(error.code, "effect_dispatch_archive");
        let (state, actions, audits): (String, i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT state FROM effect_outbox WHERE id=?",
                        params![effect.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM actions WHERE dedupe_key='atomic-effect'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM audit_events WHERE event='effect_dispatched'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!((state.as_str(), actions, audits), ("processing", 0, 0));

        database
            .with_connection(|connection| {
                connection.execute_batch("DROP TRIGGER fail_effect_audit")
            })
            .unwrap();
        assert!(database
            .archive_effect_dispatch(
                effect.id,
                "succeeded",
                "",
                &action.receipt_json,
                Some(&action),
                &audit,
            )
            .unwrap());
        assert!(!database
            .archive_effect_dispatch(
                effect.id,
                "succeeded",
                "",
                &action.receipt_json,
                Some(&action),
                &audit,
            )
            .unwrap());
        let (state, actions, audits): (String, i64, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT state FROM effect_outbox WHERE id=?",
                        params![effect.id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM actions WHERE dedupe_key='atomic-effect'",
                        [],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM audit_events WHERE event='effect_dispatched'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!((state.as_str(), actions, audits), ("succeeded", 1, 1));
    }

    #[test]
    fn rules_and_actions_round_trip() {
        let database = database();
        let rule = ModerationRule {
            id: 0,
            account_id: "a".into(),
            group_id: 0,
            rule_type: "machine".into(),
            scope: "global".into(),
            group_ids: Vec::new(),
            priority_level: "medium".into(),
            whitelist_user_ids: Vec::new(),
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
            exempt_roles: Vec::new(),
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
            receipt_json: r#"{"status":"failed"}"#.into(),
            dedupe_key: "message:9:rule:3:action:recall".into(),
            created_at: now,
        };
        let first = database.record_action(&action).unwrap();
        assert!(!database.action_succeeded("a", &action.dedupe_key).unwrap());
        action.success = true;
        action.error.clear();
        action.receipt_json = r#"{"requestId":"request-2","status":"succeeded"}"#.into();
        let second = database.record_action(&action).unwrap();
        assert_eq!(first, second);
        assert!(database.action_succeeded("a", &action.dedupe_key).unwrap());
        let receipt: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT receipt_json FROM actions WHERE id=?",
                    params![first],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(receipt.contains("request-2"));
    }

    #[test]
    fn legacy_cooldown_value_does_not_change_v2_rule_shape() {
        let database = database();
        let rule_id = database
            .save_rule(&ModerationRule {
                id: 0,
                account_id: "a".into(),
                group_id: 1,
                rule_type: "machine".into(),
                scope: "selected".into(),
                group_ids: vec![1],
                priority_level: "high".into(),
                whitelist_user_ids: vec![7],
                name: "legacy-cooldown".into(),
                matcher: "contains".into(),
                pattern: "x".into(),
                threshold: 0,
                count: 0,
                window_seconds: 0,
                cooldown_seconds: 60,
                priority: 1,
                mode: "automatic".into(),
                enabled: true,
                semantic_threshold: 0.8,
                exempt_roles: Vec::new(),
                exempt_user_ids: Vec::new(),
                actions: Vec::new(),
            })
            .unwrap();
        let rule = database
            .list_rules("a", Some(1))
            .unwrap()
            .into_iter()
            .find(|rule| rule.id == rule_id)
            .unwrap();
        assert_eq!(rule.rule_type, "machine");
        assert_eq!(rule.scope, "selected");
        assert_eq!(rule.group_ids, vec![1]);
        assert_eq!(rule.priority_level, "high");
        assert_eq!(rule.whitelist_user_ids, vec![7]);
    }

    #[test]
    fn task_reminder_is_completed_only_after_success() {
        let database = database();
        let now = Utc::now();
        let task_id = database
            .save_task(&TaskItem {
                id: 0,
                account_id: "a".into(),
                group_id: 1,
                title: "remind".into(),
                description: String::new(),
                status: "pending".into(),
                assignee_id: 2,
                created_by: 1,
                due_at: None,
                reminder_at: Some(now - chrono::Duration::minutes(1)),
                reminder_sent_at: None,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        let claimed = database.claim_due_task_reminders(now, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        database
            .finish_task_reminder(task_id, false, "temporary")
            .unwrap();
        let (sent_at, state): (Option<String>, String) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT reminder_sent_at,reminder_state FROM tasks WHERE id=?",
                    params![task_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert!(sent_at.is_none());
        assert_eq!(state, "retry");
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE tasks SET reminder_next_attempt_at=? WHERE id=?",
                    params![now.to_rfc3339(), task_id],
                )
            })
            .unwrap();
        assert_eq!(database.claim_due_task_reminders(now, 10).unwrap().len(), 1);
        database.finish_task_reminder(task_id, true, "").unwrap();
        let task = database.list_tasks("a", Some(1)).unwrap().pop().unwrap();
        assert!(task.reminder_sent_at.is_some());
        assert!(database
            .claim_due_task_reminders(Utc::now(), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn ai_and_summary_runs_are_unique() {
        let database = database();
        assert!(database.claim_ai_run("a", 1, "message:1").unwrap());
        assert!(!database.claim_ai_run("a", 1, "message:1").unwrap());
        database.finish_ai_run("a", "message:1", true, "").unwrap();
        assert_eq!(
            database.ai_run("a", "message:1").unwrap().unwrap().state,
            "succeeded"
        );
        assert!(!database.claim_ai_run("a", 1, "message:1").unwrap());

        assert!(database.claim_ai_run("a", 1, "message:failed").unwrap());
        database
            .fail_ai_run_terminal("a", "message:failed", "provider timeout")
            .unwrap();
        let failed = database.ai_run("a", "message:failed").unwrap().unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(failed.last_error, "provider timeout");
        assert!(!database.claim_ai_run("a", 1, "message:failed").unwrap());

        assert!(database.claim_summary_run("a", 1, "2026-07-21").unwrap());
        assert!(!database.claim_summary_run("a", 1, "2026-07-21").unwrap());
        database
            .finish_summary_run("a", 1, "2026-07-21", true, "")
            .unwrap();
        assert_eq!(
            database
                .summary_run("a", 1, "2026-07-21")
                .unwrap()
                .unwrap()
                .state,
            "succeeded"
        );
    }

    #[test]
    fn knowledge_chunks_replace_as_one_transaction() {
        let database = database();
        let base_id = database
            .create_knowledge_base(&KnowledgeBase {
                id: 0,
                account_id: "a".into(),
                name: "base".into(),
                description: String::new(),
                enabled: true,
                built_in: false,
                read_only: false,
            })
            .unwrap();
        let document_id = database
            .upsert_knowledge_document(&KnowledgeDocument {
                id: 0,
                base_id,
                base_name: "base".into(),
                title: "doc".into(),
                kind: "text".into(),
                content: "one two".into(),
                source: String::new(),
                content_hash: "doc-hash".into(),
                enabled: true,
            })
            .unwrap();
        database
            .replace_knowledge_chunks(
                document_id,
                &[
                    KnowledgeChunk {
                        id: 0,
                        document_id,
                        chunk_index: 0,
                        content: "one".into(),
                        content_hash: "one".into(),
                        token_count: 1,
                        enabled: true,
                    },
                    KnowledgeChunk {
                        id: 0,
                        document_id,
                        chunk_index: 1,
                        content: "two".into(),
                        content_hash: "two".into(),
                        token_count: 1,
                        enabled: false,
                    },
                ],
            )
            .unwrap();
        assert_eq!(
            database
                .list_knowledge_chunks(document_id, false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            database
                .list_knowledge_chunks(document_id, true)
                .unwrap()
                .len(),
            1
        );
    }
}
