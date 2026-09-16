//! Shared service boundary for non-desktop hosts.
use crate::{database::DatabaseExecutor, error::AppResult, gateway::RuntimeGateway, models::Group};
use std::sync::Arc;

#[cfg(feature = "headless")]
use crate::{
    models::{AuditEvent, Message, ModerationRule, Page},
    AuditQuery, MessageQuery,
};

#[derive(Clone)]
pub struct BusinessService {
    pub database: DatabaseExecutor,
    pub gateway: Arc<dyn RuntimeGateway>,
}

impl BusinessService {
    pub async fn list_groups(&self) -> AppResult<Vec<Group>> {
        let (_, account_id) = self.gateway.session_identity().await?;
        let now = chrono::Utc::now();
        self.database
            .upsert_account(crate::models::Account {
                id: account_id.clone(),
                display_name: account_id.clone(),
                role: "unknown".into(),
                discovered_at: now,
                updated_at: now,
            })
            .await?;
        self.database
            .ensure_account_defaults(account_id.clone())
            .await?;
        let live = self.gateway.list_groups().await?;
        let ids: std::collections::HashSet<_> = live.iter().map(|g| g.group_id).collect();
        for group in live {
            self.database.upsert_group(group).await?;
        }
        Ok(self.database.list_groups(Some(account_id)).await?.into_iter().filter(|g| ids.contains(&g.group_id)).collect())
    }

    #[cfg(feature = "headless")]
    pub async fn query_messages(&self, mut query: MessageQuery) -> AppResult<Page<Message>> {
        let limit = query.limit.unwrap_or(50).clamp(1, 200);
        query.limit = Some(limit + 1);
        let mut items = self.database.query_messages(query).await?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|m| m.id.to_string())
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    #[cfg(feature = "headless")]
    pub async fn query_audit(&self, mut query: AuditQuery) -> AppResult<Page<AuditEvent>> {
        let limit = query.limit.unwrap_or(100).clamp(1, 500);
        query.limit = Some(limit + 1);
        let mut items = self.database.query_audit(query).await?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items.last().map(|item| item.id.to_string())
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    #[cfg(feature = "headless")]
    pub async fn list_rules(
        &self,
        account_id: String,
        group_id: Option<i64>,
    ) -> AppResult<Vec<ModerationRule>> {
        self.database.list_rules(account_id, group_id).await
    }
}
