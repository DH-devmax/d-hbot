//! 群名片批量改名相关命令。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) async fn preview_card_names(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    prefix: String,
) -> AppResult<CardPreview> {
    require_manager(&state, group_id).await?;
    let (self_id, _) = state.gateway.session_identity().await?;
    cardnames::preview(
        group_id,
        &prefix,
        state
            .database_executor
            .list_members(account_id, group_id)
            .await?,
        self_id,
    )
}

#[tauri::command]
pub(crate) async fn apply_card_names(
    state: State<'_, AppState>,
    group_id: i64,
    plans: Vec<CardPlan>,
) -> AppResult<usize> {
    let roster = require_member_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let mut queued = 0;
    for mut plan in plans
        .into_iter()
        .filter(|plan| plan.status == "planned" && !plan.suggested_name.is_empty())
    {
        plan.member = canonical_member(&roster, plan.member.user_id)?;
        if state
            .database_executor
            .enqueue_card_job(account_id.clone(), group_id, plan, false)
            .await?
        {
            queued += 1;
            let work_id = runtime::runtime_lane_id("cardRename", &account_id, group_id);
            state.runtime_coordination.tracker.enqueue(
                &work_id,
                "cardRename",
                "批量修改群名片",
                "群名片队列",
            );
        }
    }
    if queued > 0 {
        state.runtime_coordination.notify();
    }
    Ok(queued)
}

#[tauri::command]
pub(crate) async fn list_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    limit: Option<usize>,
) -> AppResult<Vec<CardRenameJob>> {
    state
        .database_executor
        .list_card_jobs(account_id, group_id, limit.unwrap_or(200))
        .await
}

#[tauri::command]
pub(crate) async fn retry_card_rename_jobs(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<usize> {
    require_member_manager(&state, group_id).await?;
    let current_account = state.gateway.session_identity().await?.1;
    if current_account != account_id {
        return Err(AppError::new(
            "account_mismatch",
            "只能重试当前已登录账号的名片任务",
        ));
    }
    let retried = state
        .database_executor
        .retry_failed_card_jobs(account_id.clone(), group_id)
        .await?;
    if retried > 0 {
        let work_id = runtime::runtime_lane_id("cardRename", &account_id, group_id);
        for _ in 0..retried {
            state.runtime_coordination.tracker.enqueue(
                &work_id,
                "cardRename",
                "批量修改群名片",
                "群名片队列",
            );
        }
        state.runtime_coordination.notify();
    }
    Ok(retried)
}

#[tauri::command]
pub(crate) async fn get_card_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let prefix_key = format!("card.prefix.{account_id}.{group_id}");
    let auto_key = format!("card.auto.{account_id}.{group_id}");
    let paused_key = format!("card.paused.{account_id}.{group_id}");
    Ok(serde_json::json!({
        "prefix": state.database_executor.get_setting(prefix_key).await?.unwrap_or_else(|| "DH".into()),
        "autoRename": state.database_executor.get_setting(auto_key).await?.as_deref() == Some("true"),
        "paused": state.database_executor.get_setting(paused_key).await?.as_deref() == Some("true")
    }))
}

#[tauri::command]
pub(crate) async fn save_card_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    prefix: String,
    auto_rename: bool,
    paused: bool,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    let prefix = if prefix.trim().is_empty() {
        "DH"
    } else {
        prefix.trim()
    };
    state
        .database_executor
        .set_setting(
            format!("card.prefix.{account_id}.{group_id}"),
            prefix.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("card.auto.{account_id}.{group_id}"),
            if auto_rename { "true" } else { "false" }.into(),
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            format!("card.paused.{account_id}.{group_id}"),
            if paused { "true" } else { "false" }.into(),
            false,
        )
        .await
}

#[tauri::command]
pub(crate) async fn save_group_welcome(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    welcome_message: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    state
        .database_executor
        .set_group_welcome(account_id, group_id, welcome_message)
        .await
}
