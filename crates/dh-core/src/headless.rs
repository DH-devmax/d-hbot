//! Desktop business handlers hosted without a Tauri runtime.
pub(crate) use crate::models::KnowledgeChunk as StoredKnowledgeChunk;
use crate::*;
pub(crate) use crate::{
    business_apps::{
        BusinessAppContext, BusinessAppHealth, BusinessAppRegistry, PREDICTION_APP_ID,
    },
    database::DatabaseExecutor,
    diagnostics::redact,
    error::{AppError, AppResult},
    gateway::{GatewayReceipt, RuntimeGateway},
    models::*,
    prediction::PredictionSource,
    runtime_work::RuntimeCoordination,
    secrets::SecretStore,
};
pub(crate) use chrono::Utc;
pub(crate) use serde::{Deserialize, Serialize};
use std::{ops::Deref, sync::Arc, time::Duration};

pub struct State<'a, T>(pub &'a T);
impl<T> Copy for State<'_, T> {}
impl<T> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

#[derive(Clone)]
pub struct CommandEventHandle(pub tokio::sync::broadcast::Sender<serde_json::Value>);
impl CommandEventHandle {
    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> AppResult<()> {
        let value = serde_json::to_value(payload)
            .map_err(|_| AppError::new("event_encode", "事件编码失败"))?;
        let _ = self
            .0
            .send(serde_json::json!({"event":event,"payload":value}));
        Ok(())
    }
}
impl crate::runtime_events::RuntimeEventSink for CommandEventHandle {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = CommandEventHandle::emit(self, event, payload);
    }
}
pub struct AppState {
    pub database_executor: DatabaseExecutor,
    pub secrets: SecretStore,
    pub gateway: Arc<dyn RuntimeGateway>,
    pub ai_pool: ai::AiProviderPool,
    pub prediction_source: Arc<dyn PredictionSource>,
    pub runtime_coordination: RuntimeCoordination,
    pub events: CommandEventHandle,
    pub logger: diagnostics::Logger,
}
impl AppState {
    pub fn new(
        database_executor: DatabaseExecutor,
        secrets: SecretStore,
        gateway: Arc<dyn RuntimeGateway>,
        logger: diagnostics::Logger,
    ) -> AppResult<Self> {
        Ok(Self {
            database_executor,
            secrets,
            gateway,
            logger,
            ai_pool: ai::AiProviderPool::default(),
            prediction_source: Arc::new(prediction::PublicLotterySource::new(
                Duration::from_secs(8),
            )?),
            runtime_coordination: RuntimeCoordination::default(),
            events: CommandEventHandle(tokio::sync::broadcast::channel(256).0),
        })
    }
}
pub(crate) fn default_reasoning_effort() -> String {
    "low".into()
}
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum GroupBatchAction {
    Announcement,
    Mute,
    Unmute,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupBatchInput {
    action: GroupBatchAction,
    group_ids: Vec<i64>,
    text: Option<String>,
}

include!("../../../tauri3/src-tauri/src/command_support.rs");
include!("web_dispatch.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn web_dispatch_preserves_crud_chunking_and_account_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = paths::AppPaths {
            root: root.clone(),
            v3: root.clone(),
            database: root.join("db"),
            secrets: root.join("secrets"),
            logs: root.join("logs"),
            legacy_backups: root.join("backups"),
            runtime_mode_file: root.join("mode"),
        };
        paths.prepare().unwrap();
        let gateway = Arc::new(fixture::FixtureGateway::new_default());
        let state = AppState::new(
            DatabaseExecutor::start(database::Database::open(&paths).unwrap()).unwrap(),
            SecretStore::new(paths.secrets.clone()),
            gateway,
            diagnostics::Logger::new(paths.logs.clone()),
        )
        .unwrap();
        let groups = dispatch(&state, "list_groups", json!({})).await.unwrap();
        let account = groups[0]["accountId"].as_str().unwrap();
        let group = groups[0]["groupId"].as_i64().unwrap();
        let roster = state.gateway.list_members(group).await.unwrap();
        use crate::shared_commands::groups::validate_roster_context;
        assert!(validate_roster_context(&roster.members, account, group, 1, 1).is_ok());
        assert!(validate_roster_context(&roster.members, account, group, 1, 2).is_err());
        assert!(validate_roster_context(&roster.members, "another-account", group, 1, 1).is_err());
        assert!(validate_roster_context(&roster.members, account, group + 1, 1, 1).is_err());
        let mut base = json!({"id":0,"accountId":account,"name":"Web test","description":"synthetic","enabled":true,"builtIn":false,"readOnly":false});
        let id = dispatch(&state, "create_knowledge_base", json!({"base":base}))
            .await
            .unwrap();
        base["id"] = id.clone();
        base["name"] = json!("Updated");
        dispatch(&state, "update_knowledge_base", json!({"base":base}))
            .await
            .unwrap();
        let rows = dispatch(&state, "list_knowledge_bases", json!({"accountId":account}))
            .await
            .unwrap();
        assert!(rows
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == id && row["name"] == "Updated"));
        let document=dispatch(&state,"save_knowledge_document",json!({"document":{"id":0,"baseId":id,"baseName":"Updated","title":"Web document","kind":"text","content":"Synthetic knowledge for Web dispatch.","source":"manual","contentHash":"","enabled":true}})).await.unwrap();
        let docs = dispatch(&state, "list_knowledge_documents", json!({"baseId":id}))
            .await
            .unwrap();
        assert_eq!(docs.as_array().unwrap().len(), 1);
        assert_eq!(docs[0]["id"], document);
        base["accountId"] = json!("another-account");
        assert_eq!(
            dispatch(&state, "update_knowledge_base", json!({"base":base}))
                .await
                .unwrap_err()
                .code,
            "account_mismatch"
        );
        assert_eq!(
            dispatch(
                &state,
                "delete_knowledge_base",
                json!({"accountId":account,"baseId":"wrong-type"})
            )
            .await
            .unwrap_err()
            .code,
            "invalid_argument"
        );
        dispatch(
            &state,
            "delete_knowledge_base",
            json!({"accountId":account,"baseId":id}),
        )
        .await
        .unwrap();
        let rows = dispatch(&state, "list_knowledge_bases", json!({"accountId":account}))
            .await
            .unwrap();
        assert!(!rows.as_array().unwrap().iter().any(|row| row["id"] == id));
    }

    #[tokio::test]
    async fn ordinary_group_member_can_send_without_manager_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = paths::AppPaths {
            root: root.clone(),
            v3: root.clone(),
            database: root.join("db"),
            secrets: root.join("secrets"),
            logs: root.join("logs"),
            legacy_backups: root.join("backups"),
            runtime_mode_file: root.join("mode"),
        };
        paths.prepare().unwrap();
        let gateway = Arc::new(fixture::FixtureGateway::new_default());
        let state = AppState::new(
            DatabaseExecutor::start(database::Database::open(&paths).unwrap()).unwrap(),
            SecretStore::new(paths.secrets.clone()),
            gateway.clone(),
            diagnostics::Logger::new(paths.logs.clone()),
        )
        .unwrap();

        dispatch(&state, "list_groups", json!({})).await.unwrap();

        gateway
            .member_updated(
                fixture::FIXTURE_GROUP,
                10001,
                None,
                Some("member".into()),
                None,
                None,
            )
            .await
            .unwrap();

        let message_id = dispatch(
            &state,
            "send_text",
            json!({"groupId": fixture::FIXTURE_GROUP, "text": "ordinary member message"}),
        )
        .await
        .unwrap();
        assert_eq!(message_id, json!("fixture-delivery-1"));

        let management = dispatch(
            &state,
            "execute_group_batch",
            json!({
                "input": {
                    "action": "announcement",
                    "groupIds": [fixture::FIXTURE_GROUP],
                    "text": "must remain manager-only"
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(management[0]["success"], false);
        assert_eq!(management[0]["status"], "failed");
        assert!(management[0]["error"].as_str().unwrap().contains("权限"));

        let actions = gateway.snapshot().await.actions;
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "send_text");
    }
}
