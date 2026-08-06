//! 群日报设置与生成。

use tauri::State;

use crate::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SummarySettings {
    account_id: String,
    enabled: bool,
    time: String,
    group_ids: Vec<i64>,
    timezone: String,
}

#[tauri::command]
pub(crate) async fn list_daily_summaries(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<DailySummary>> {
    state
        .database_executor
        .list_daily_summaries(account_id, limit.unwrap_or(30))
        .await
}

#[tauri::command]
pub(crate) async fn get_summary_settings(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<SummarySettings> {
    let enabled = state
        .database_executor
        .get_setting(format!("summary.enabled.{account_id}"))
        .await?
        .as_deref()
        == Some("true");
    let time = state
        .database_executor
        .get_setting(format!("summary.time.{account_id}"))
        .await?
        .unwrap_or_else(|| "23:00".into());
    let group_ids = state
        .database_executor
        .get_setting(format!("summary.groups.{account_id}"))
        .await?
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    Ok(SummarySettings {
        account_id,
        enabled,
        time,
        group_ids,
        timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
    })
}

#[tauri::command]
pub(crate) async fn save_summary_settings(
    state: State<'_, AppState>,
    settings: SummarySettings,
) -> AppResult<()> {
    require_account(&state, &settings.account_id).await?;
    chrono::NaiveTime::parse_from_str(&settings.time, "%H:%M")
        .map_err(|_| AppError::new("summary_time", "摘要时间必须使用 HH:MM 格式"))?;
    for group_id in &settings.group_ids {
        require_manager(&state, *group_id).await?;
    }
    state
        .database_executor
        .set_setting(
            format!("summary.enabled.{}", settings.account_id),
            if settings.enabled { "true" } else { "false" }.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("summary.time.{}", settings.account_id),
            settings.time,
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("summary.groups.{}", settings.account_id),
            serde_json::to_string(&settings.group_ids)
                .map_err(|error| AppError::new("summary_settings", error.to_string()))?,
            false,
        )
        .await
}

#[tauri::command]
pub(crate) async fn generate_daily_summary(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<DailySummary> {
    require_manager(&state, group_id).await?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let messages = state
        .database_executor
        .list_messages(account_id.clone(), Some(group_id), 1000)
        .await?
        .into_iter()
        .filter(|message| {
            message
                .sent_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
                == today
        })
        .take(200)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        let error = AppError::new("summary_empty", "当前群今天还没有可供总结的消息");
        let _ = state
            .database_executor
            .record_audit(AuditEvent {
                id: 0,
                account_id,
                group_id,
                user_id: 0,
                actor: "当前管理员".into(),
                event: "daily_summary_failed".into(),
                level: "error".into(),
                details: error.message.clone(),
                created_at: Utc::now(),
            })
            .await;
        return Err(error);
    }
    let provider = state.ai_pool.chain(
        ai_endpoint_configs(&state, &account_id, None)
            .await?
            .into_iter()
            .map(|(_, config)| config)
            .collect(),
    )?;
    let group_name = state
        .database_executor
        .list_groups(Some(account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .map(|group| group.name)
        .unwrap_or_default();
    let request = ai::AiRequest {
        version: "1",
        event_id: uuid::Uuid::new_v4().to_string(),
        persona: ai::PERSONA,
        group_id,
        group_name,
        member_id: 1,
        member_name: "本地摘要任务".into(),
        member_role: "admin".into(),
        message_id: uuid::Uuid::new_v4().to_string(),
        message: "请用简短中文总结今天的重要信息、待办和需要人工确认的事项。摘要只保存在总览，不生成任何群管动作。".into(),
        recent_context: messages
            .into_iter()
            .rev()
            .map(|message| ai::AiContextMessage {
                user_id: message.user_id,
                name: message.sender_name,
                text: message.text,
                time: message.sent_at.timestamp(),
            })
            .collect(),
        knowledge: Vec::new(),
    };
    let decision = match ai::AiProvider::decide(provider.as_ref(), &request).await {
        Ok(decision) => decision,
        Err(error) => {
            let _ = state
                .database_executor
                .record_audit(AuditEvent {
                    id: 0,
                    account_id,
                    group_id,
                    user_id: 0,
                    actor: "当前管理员".into(),
                    event: "daily_summary_failed".into(),
                    level: "error".into(),
                    details: error.message.clone(),
                    created_at: Utc::now(),
                })
                .await;
            return Err(error);
        }
    };
    let summary = DailySummary {
        id: 0,
        account_id,
        group_id,
        local_date: today,
        content: decision.reply,
        source: "ai".into(),
        created_at: Utc::now(),
    };
    let id = state
        .database_executor
        .save_daily_summary(summary.clone())
        .await?;
    let _ = state
        .database_executor
        .record_audit(AuditEvent {
            id: 0,
            account_id: summary.account_id.clone(),
            group_id,
            user_id: 0,
            actor: "当前管理员".into(),
            event: "daily_summary".into(),
            level: "success".into(),
            details: format!("已生成 {} 的本机每日摘要", summary.local_date),
            created_at: Utc::now(),
        })
        .await;
    Ok(DailySummary { id, ..summary })
}
