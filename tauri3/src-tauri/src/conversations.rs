//! Account-scoped conversation ledger. Protocol identifiers and raw envelopes
//! stay in SQLite; Web views use opaque local IDs and display names only.
use crate::{
    database::Database,
    error::{AppError, AppResult},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_recalls (
 account_id TEXT NOT NULL, target TEXT NOT NULL, server_id TEXT NOT NULL, source TEXT NOT NULL,
 PRIMARY KEY(account_id,target,server_id)
);
CREATE TABLE IF NOT EXISTS conversations (
 id TEXT PRIMARY KEY, platform TEXT NOT NULL, account_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('group','private')), target TEXT NOT NULL,
 name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 0,
 manual_takeover INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL,
 UNIQUE(platform,account_id,kind,target)
);
CREATE TABLE IF NOT EXISTS conversation_messages (
 id INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id TEXT NOT NULL REFERENCES conversations(id),
 server_id TEXT NOT NULL, sender TEXT NOT NULL, sender_name TEXT NOT NULL,
 sent_at INTEGER NOT NULL, text TEXT NOT NULL, content_kind TEXT NOT NULL,
 flow TEXT NOT NULL, state TEXT NOT NULL, reason TEXT NOT NULL,
 ack_state TEXT NOT NULL DEFAULT 'pending', raw_metadata TEXT NOT NULL,
 received_at INTEGER NOT NULL, UNIQUE(conversation_id,server_id)
);
CREATE INDEX IF NOT EXISTS conversation_history ON conversation_messages(conversation_id,id);
CREATE TABLE IF NOT EXISTS conversation_sync (
 account_id TEXT PRIMARY KEY, cursor TEXT NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS conversation_packets (
 account_id TEXT NOT NULL, packet_hash TEXT NOT NULL, service INTEGER NOT NULL,
 command INTEGER NOT NULL, body BLOB NOT NULL, reason TEXT NOT NULL,
 received_at INTEGER NOT NULL, PRIMARY KEY(account_id,packet_hash)
);
CREATE TABLE IF NOT EXISTS conversation_routes (account_id TEXT NOT NULL, group_id INTEGER NOT NULL, cloud_id TEXT NOT NULL, PRIMARY KEY(account_id,group_id), UNIQUE(account_id,cloud_id));
CREATE TABLE IF NOT EXISTS conversation_sends (client_id TEXT PRIMARY KEY,account_id TEXT NOT NULL,group_id INTEGER NOT NULL,state TEXT NOT NULL,server_id TEXT NOT NULL DEFAULT '',created_at INTEGER NOT NULL,nonce TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS conversation_audit (
 id INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id TEXT NOT NULL,
 event TEXT NOT NULL, created_at INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversationKind {
    Group,
    Private,
}
impl ConversationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Private => "private",
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct ConversationKey {
    pub platform: String,
    pub account_id: String,
    pub kind: String,
    pub target: String,
}
impl ConversationKey {
    fn validate(&self) -> AppResult<()> {
        if self.platform != "wangshangliao"
            || !matches!(self.kind.as_str(), "group" | "private")
            || self.account_id.is_empty()
            || self.target.is_empty()
        {
            return Err(AppError::new("conversation_key", "会话身份无效"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct InboundMessage {
    pub conversation: ConversationKey,
    pub conversation_name: String,
    pub server_id: String,
    pub sender: String,
    pub sender_name: String,
    pub sent_at: i64,
    pub text: String,
    pub content_kind: String,
    pub outgoing: bool,
    pub raw_metadata: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MessageDecision {
    Ignored { reason: String },
    Accepted,
    NeedsHuman { reason: String },
    Rejected { reason: String },
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeliveryResult {
    Sent {
        correlation_id: String,
        server_id: String,
    },
    Rejected {
        correlation_id: String,
        reason: String,
    },
    Unknown {
        correlation_id: String,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub manual_takeover: bool,
    pub updated_at: i64,
    pub message_count: i64,
    pub pending_ack: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: i64,
    pub sender_name: String,
    pub sent_at: i64,
    pub text: String,
    pub content_kind: String,
    pub flow: String,
    pub state: String,
    pub reason: String,
    pub ack_state: String,
    pub ai_state: String,
    pub delivery_state: String,
}
fn db_error(_: rusqlite::Error) -> AppError {
    AppError::new("conversation_database", "会话数据读写失败")
}

impl Database {
    pub fn recall_target_metadata(&self, account: &str, group: i64, user: i64, server: &str) -> AppResult<Option<String>> {
        self.with_connection(|c| c.query_row(
            "SELECT m.raw_metadata FROM conversation_messages m JOIN conversations c ON c.id=m.conversation_id JOIN conversation_routes r ON r.account_id=c.account_id AND r.cloud_id=c.target JOIN messages old ON old.account_id=r.account_id AND old.group_id=r.group_id AND old.server_message_id=m.server_id WHERE c.account_id=? AND c.kind='group' AND r.group_id=? AND old.user_id=? AND m.server_id=? AND m.state!='recalled'",
            params![account,group,user,server], |r| r.get(0)).optional()).map_err(db_error)
    }
    pub fn mark_conversation_recalled(&self, account: &str, target: &str, server: &str, source: &str) -> AppResult<()> {
        self.with_connection(|c| c.execute("INSERT OR IGNORE INTO conversation_recalls VALUES(?,?,?,?)",params![account,target,server,source])).map_err(db_error)?;
        self.with_connection(|c| c.execute(
            "UPDATE conversation_messages SET state='recalled',reason='server_recall_notification' WHERE server_id=? AND conversation_id IN (SELECT id FROM conversations WHERE account_id=? AND kind='group' AND target=?)",
            params![server,account,target])).map_err(db_error)?;
        Ok(())
    }
    pub fn conversation_recall_observed(&self, account: &str, target: &str, server: &str) -> AppResult<bool> {
        self.with_connection(|c| c.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_recalls WHERE account_id=? AND target=? AND server_id=?)",
            params![account,target,server], |r| r.get(0))).map_err(db_error)
    }
    pub fn save_conversation_route(&self, account: &str, group: i64, cloud: &str) -> AppResult<()> {
        self.with_connection(|c| c.execute("INSERT INTO conversation_routes VALUES(?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET cloud_id=excluded.cloud_id",params![account,group,cloud])).map_err(db_error)?;
        Ok(())
    }
    pub fn conversation_route(&self, account: &str, group: i64) -> AppResult<Option<String>> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT cloud_id FROM conversation_routes WHERE account_id=? AND group_id=?",
                params![account, group],
                |r| r.get(0),
            )
            .optional()
        })
        .map_err(db_error)
    }
    pub fn reserve_conversation_send(
        &self,
        account: &str,
        group: i64,
        client: &str,
        nonce: &str,
    ) -> AppResult<()> {
        // Persist unknown BEFORE writing: a crash must never trigger an automatic resend.
        self.with_connection(|c| c.execute("INSERT INTO conversation_sends(client_id,account_id,group_id,state,created_at,nonce) VALUES(?,?,?,'unknown',?,?)",params![client,account,group,chrono::Utc::now().timestamp_millis(),nonce])).map_err(db_error)?;
        Ok(())
    }
    pub fn finish_conversation_send(
        &self,
        client: &str,
        state: &str,
        server: &str,
    ) -> AppResult<()> {
        self.with_connection(|c| {
            c.execute(
                "UPDATE conversation_sends SET state=?,server_id=? WHERE client_id=?",
                params![state, server, client],
            )
        })
        .map_err(db_error)?;
        Ok(())
    }
    pub fn conversation_gateway_batch(
        &self,
        account: &str,
    ) -> AppResult<crate::gateway::GatewayBatch> {
        let session = format!("rust:{account}");
        let records = self.with_connection(|c| {
            let mut q=c.prepare("SELECT m.id,m.raw_metadata FROM conversation_messages m JOIN conversations c ON c.id=m.conversation_id WHERE c.account_id=? AND c.kind='group' AND m.state='pending' AND m.flow='in' AND m.content_kind='text' AND json_type(m.raw_metadata,'$.decoded')='object' ORDER BY m.id LIMIT 100")?;
            let rows=q.query_map([account],|r| Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?;
            rows.collect::<Result<Vec<_>,_>>()
        }).map_err(db_error)?.into_iter().map(|(sequence,raw)| {
            let payload=serde_json::from_str(&raw).map_err(|_| AppError::new("message_metadata","消息索引无效"))?;
            Ok(crate::gateway::GatewayRecord {session:session.clone(),sequence,kind:crate::gateway::GatewayRecordKind::Message,source:"rust-nim".into(),payload})
        }).collect::<AppResult<Vec<_>>>()?;
        Ok(crate::gateway::GatewayBatch {
            session,
            records,
            remaining: 0,
            dropped: 0,
        })
    }
    pub fn acknowledge_conversation_gateway(&self, account: &str, sequence: u64) -> AppResult<()> {
        self.with_connection(|c| c.execute("UPDATE conversation_messages SET state='queued' WHERE id<=? AND state='pending' AND content_kind='text' AND flow='in' AND json_type(raw_metadata,'$.decoded')='object' AND conversation_id IN (SELECT id FROM conversations WHERE account_id=? AND kind='group')",params![sequence,account])).map_err(db_error)?;
        Ok(())
    }
    pub fn ingest_conversation(&self, message: &InboundMessage) -> AppResult<bool> {
        message.conversation.validate()?;
        if message.server_id.is_empty()
            || message.raw_metadata.len() > 1024 * 1024
            || message.text.len() > 65536
        {
            return Err(AppError::new("conversation_message", "消息身份或长度无效"));
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        let tx = connection.transaction().map_err(db_error)?;
        let key = &message.conversation;
        let now = chrono::Utc::now().timestamp_millis();
        let fallback_name = if key.kind == "group" {
            "群聊"
        } else {
            "私信"
        };
        let name = if message.conversation_name.trim().is_empty() {
            fallback_name
        } else {
            &message.conversation_name
        };
        tx.execute("INSERT INTO conversations(id,platform,account_id,kind,target,name,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(platform,account_id,kind,target) DO UPDATE SET updated_at=excluded.updated_at,name=CASE WHEN excluded.name IN ('群聊','私信') THEN conversations.name ELSE excluded.name END",
            params![Uuid::new_v4().to_string(),key.platform,key.account_id,key.kind,key.target,name,now]).map_err(db_error)?;
        let id: String = tx.query_row("SELECT id FROM conversations WHERE platform=? AND account_id=? AND kind=? AND target=?",
            params![key.platform,key.account_id,key.kind,key.target], |r| r.get(0)).map_err(db_error)?;
        let (state, reason) = if message.outgoing {
            ("ignored", "本人消息")
        } else if message.content_kind == "encrypted" {
            ("needs_human", "消息内容解码尚未通过验证")
        } else if message.content_kind != "text" {
            ("needs_human", "暂不支持此消息类型")
        } else {
            ("pending", "")
        };
        let count = tx.execute("INSERT INTO conversation_messages(conversation_id,server_id,sender,sender_name,sent_at,text,content_kind,flow,state,reason,raw_metadata,received_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(conversation_id,server_id) DO NOTHING",
            params![id,message.server_id,message.sender,message.sender_name,message.sent_at,message.text,message.content_kind,if message.outgoing {"out"} else {"in"},state,reason,message.raw_metadata,now]).map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(count == 1)
    }

    pub fn list_conversations(&self, account: &str) -> AppResult<Vec<ConversationView>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        let mut query = connection.prepare("SELECT c.id,c.kind,c.name,c.enabled,c.manual_takeover,c.updated_at,COUNT(m.id),COALESCE(SUM(m.ack_state='pending'),0) FROM conversations c LEFT JOIN conversation_messages m ON m.conversation_id=c.id WHERE c.account_id=? GROUP BY c.id ORDER BY c.updated_at DESC LIMIT 200").map_err(db_error)?;
        let rows = query
            .query_map([account], |r| {
                Ok(ConversationView {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    enabled: r.get(3)?,
                    manual_takeover: r.get(4)?,
                    updated_at: r.get(5)?,
                    message_count: r.get(6)?,
                    pending_ack: r.get(7)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    pub fn conversation_history(
        &self,
        account: &str,
        id: &str,
        before: i64,
    ) -> AppResult<Vec<MessageView>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        let mut query = connection.prepare("SELECT m.id,m.sender_name,m.sent_at,m.text,m.content_kind,m.flow,COALESCE(r.processing_state,m.state),m.reason,m.ack_state,COALESCE(a.state,''),COALESCE(e.state,'') FROM conversation_messages m JOIN conversations c ON c.id=m.conversation_id LEFT JOIN messages r ON r.account_id=c.account_id AND r.group_id=json_extract(CASE WHEN json_valid(m.raw_metadata) THEN m.raw_metadata ELSE '{}' END,'$.decoded.to.id') AND r.server_message_id=m.server_id LEFT JOIN ai_runs a ON a.account_id=c.account_id AND a.run_key='message:'||r.id LEFT JOIN effect_outbox e ON e.account_id=c.account_id AND e.dedupe_key='ai-reply:'||r.id WHERE c.account_id=? AND c.id=? AND (?=0 OR m.id<?) ORDER BY m.id DESC LIMIT 100").map_err(db_error)?;
        let rows = query
            .query_map(params![account, id, before, before], |r| {
                Ok(MessageView {
                    id: r.get(0)?,
                    sender_name: r.get(1)?,
                    sent_at: r.get(2)?,
                    text: if r.get::<_, String>(7)? == "server_recall_notification" { "消息已撤回".into() } else { r.get(3)? },
                    content_kind: r.get(4)?,
                    flow: r.get(5)?,
                    state: if r.get::<_, String>(7)? == "server_recall_notification" { "recalled".into() } else { r.get(6)? },
                    reason: r.get(7)?,
                    ack_state: r.get(8)?,
                    ai_state: r.get(9)?,
                    delivery_state: r.get(10)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
    }

    pub fn set_conversation_policy(
        &self,
        account: &str,
        id: &str,
        enabled: bool,
        takeover: bool,
    ) -> AppResult<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        let tx = connection.transaction().map_err(db_error)?;
        if tx
            .execute(
                "UPDATE conversations SET enabled=?,manual_takeover=? WHERE id=? AND account_id=?",
                params![enabled, takeover, id, account],
            )
            .map_err(db_error)?
            != 1
        {
            return Err(AppError::new(
                "conversation_missing",
                "会话不存在或不属于当前账号",
            ));
        }
        if enabled {
            let kind: String = tx
                .query_row(
                    "SELECT kind FROM conversations WHERE id=? AND account_id=?",
                    params![id, account],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            if kind != "group" {
                return Err(AppError::new("private_policy", "私信自动回复尚未开放"));
            }
        }
        tx.execute("UPDATE groups SET manual_takeover=? WHERE account_id=? AND group_id IN (SELECT r.group_id FROM conversation_routes r JOIN conversations c ON c.account_id=r.account_id AND c.target=r.cloud_id AND c.kind='group' WHERE c.id=? AND c.account_id=?)",params![takeover,account,id,account]).map_err(db_error)?;
        tx.execute(
            "INSERT INTO conversation_audit(conversation_id,event,created_at) VALUES(?,?,?)",
            params![
                id,
                format!("policy:enabled={enabled},manual_takeover={takeover}"),
                chrono::Utc::now().timestamp_millis()
            ],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub fn mark_conversation_ack(&self, key: &ConversationKey, server_id: &str) -> AppResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        connection.execute("UPDATE conversation_messages SET ack_state='flushed' WHERE server_id=? AND conversation_id IN (SELECT id FROM conversations WHERE platform=? AND account_id=? AND kind=? AND target=?)",params![server_id,key.platform,key.account_id,key.kind,key.target]).map_err(db_error)?;
        Ok(())
    }

    pub fn journal_nim_packet(
        &self,
        account: &str,
        service: u8,
        command: u8,
        body: &[u8],
        reason: &str,
    ) -> AppResult<()> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update([service, command]);
        hash.update(body);
        let hash = format!("{:x}", hash.finalize());
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        connection.execute("INSERT OR IGNORE INTO conversation_packets(account_id,packet_hash,service,command,body,reason,received_at) VALUES(?,?,?,?,?,?,?)",params![account,hash,service,command,body,reason,chrono::Utc::now().timestamp_millis()]).map_err(db_error)?;
        Ok(())
    }

    pub fn conversation_cursor(&self, account: &str) -> AppResult<u64> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        let value: Option<String> = connection
            .query_row(
                "SELECT cursor FROM conversation_sync WHERE account_id=?",
                [account],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        value
            .map(|v| {
                v.parse()
                    .map_err(|_| AppError::new("sync_cursor", "同步游标无效"))
            })
            .unwrap_or(Ok(0))
    }

    pub fn save_conversation_cursor(&self, account: &str, cursor: u64) -> AppResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| AppError::new("conversation_database", "会话数据库不可用"))?;
        connection.execute("INSERT INTO conversation_sync(account_id,cursor,updated_at) VALUES(?,?,?) ON CONFLICT(account_id) DO UPDATE SET cursor=excluded.cursor,updated_at=excluded.updated_at",params![account,cursor.to_string(),chrono::Utc::now().timestamp_millis()]).map_err(db_error)?;
        Ok(())
    }
}

pub fn initialize(connection: &Connection) -> AppResult<()> {
    connection.execute_batch(SCHEMA).map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> Database {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection.execute_batch("CREATE TABLE groups(account_id TEXT,group_id INTEGER,manual_takeover INTEGER); CREATE TABLE messages(id INTEGER,account_id TEXT,group_id INTEGER,server_message_id TEXT,processing_state TEXT); CREATE TABLE ai_runs(account_id TEXT,run_key TEXT,state TEXT); CREATE TABLE effect_outbox(account_id TEXT,dedupe_key TEXT,state TEXT)").unwrap();
        Database {
            path: Default::default(),
            connection: std::sync::Arc::new(std::sync::Mutex::new(connection)),
        }
    }
    fn message(account: &str, kind: &str) -> InboundMessage {
        InboundMessage {
            conversation: ConversationKey {
                platform: "wangshangliao".into(),
                account_id: account.into(),
                kind: kind.into(),
                target: "same-target".into(),
            },
            conversation_name: "Test".into(),
            server_id: "same-message".into(),
            sender: "secret-sender".into(),
            sender_name: "Alice".into(),
            sent_at: 1,
            text: "hello".into(),
            content_kind: "text".into(),
            outgoing: false,
            raw_metadata: "secret-envelope".into(),
        }
    }
    #[test]
    fn recall_notification_is_scoped_to_account_group_and_message() {
        let db = database();
        db.ingest_conversation(&message("a", "group")).unwrap();
        db.ingest_conversation(&message("b", "group")).unwrap();
        db.ingest_conversation(&message("a", "private")).unwrap();
        db.mark_conversation_recalled("a", "same-target", "same-message", "notification").unwrap();
        assert!(db.conversation_recall_observed("a", "same-target", "same-message").unwrap());
        assert!(!db.conversation_recall_observed("b", "same-target", "same-message").unwrap());
        assert!(!db.conversation_recall_observed("a", "other-target", "same-message").unwrap());
        let view = db.list_conversations("a").unwrap();
        for c in view {
            let messages = db.conversation_history("a", &c.id, 0).unwrap();
            assert_eq!(messages[0].text, if c.kind == "group" { "消息已撤回" } else { "hello" });
        }
    }

    #[test]
    fn durable_gateway_replays_until_ack_and_isolates_accounts() {
        let db = database();
        let mut m = message("a", "group");
        m.raw_metadata =
            serde_json::json!({"decoded":{"msgSession":2},"idServer":"one"}).to_string();
        db.ingest_conversation(&m).unwrap();
        assert_eq!(db.conversation_gateway_batch("a").unwrap().records.len(), 1);
        assert_eq!(db.conversation_gateway_batch("a").unwrap().records.len(), 1);
        assert!(db
            .conversation_gateway_batch("b")
            .unwrap()
            .records
            .is_empty());
        let seq = db.conversation_gateway_batch("a").unwrap().records[0].sequence;
        db.acknowledge_conversation_gateway("b", seq).unwrap();
        assert_eq!(db.conversation_gateway_batch("a").unwrap().records.len(), 1);
        db.acknowledge_conversation_gateway("a", seq).unwrap();
        assert!(db
            .conversation_gateway_batch("a")
            .unwrap()
            .records
            .is_empty());
        assert!(!db.ingest_conversation(&m).unwrap());
        assert!(db
            .conversation_gateway_batch("a")
            .unwrap()
            .records
            .is_empty());
    }
    #[test]
    fn reserved_sends_survive_as_unknown_and_nonce_reuse_is_rejected() {
        let db = database();
        db.reserve_conversation_send("a", 1, "one", "8").unwrap();
        assert!(db.reserve_conversation_send("a", 1, "two", "8").is_err());
        let state: String = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT state FROM conversation_sends WHERE client_id='one'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(state, "unknown");
        db.finish_conversation_send("one", "sent", "remote")
            .unwrap();
        assert!(db.reserve_conversation_send("a", 1, "one", "9").is_err());
    }
    #[test]
    fn deduplication_and_history_are_account_and_kind_scoped() {
        let db = database();
        for (account, kind) in [("a", "group"), ("a", "private"), ("b", "group")] {
            assert!(db.ingest_conversation(&message(account, kind)).unwrap());
            assert!(!db.ingest_conversation(&message(account, kind)).unwrap());
        }
        let views = db.list_conversations("a").unwrap();
        assert_eq!(views.len(), 2);
        for view in views {
            assert_eq!(view.message_count, 1);
            assert!(db
                .conversation_history("b", &view.id, 0)
                .unwrap()
                .is_empty());
            assert!(db
                .set_conversation_policy("b", &view.id, true, false)
                .is_err());
            let json =
                serde_json::to_string(&db.conversation_history("a", &view.id, 0).unwrap()).unwrap();
            assert!(!json.contains("secret"));
            assert!(!json.contains("same-message"));
        }
    }
    #[test]
    fn opaque_and_self_messages_never_become_ai_jobs() {
        let db = database();
        let mut m = message("a", "private");
        m.content_kind = "encrypted".into();
        db.ingest_conversation(&m).unwrap();
        let id = db.list_conversations("a").unwrap().remove(0).id;
        assert_eq!(
            db.conversation_history("a", &id, 0).unwrap()[0].state,
            "needs_human"
        );
        db.mark_conversation_ack(&m.conversation, &m.server_id)
            .unwrap();
        assert_eq!(
            db.conversation_history("a", &id, 0).unwrap()[0].ack_state,
            "flushed"
        );
        m.server_id = "self".into();
        m.outgoing = true;
        db.ingest_conversation(&m).unwrap();
        assert_eq!(
            db.conversation_history("a", &id, 0).unwrap()[0].state,
            "ignored"
        );
    }
}
