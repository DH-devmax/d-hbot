//! 发送消息、批量群发与消息查询。

use tauri::State;

use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchSendResult {
    group_id: i64,
    success: bool,
    message_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupBatchResult {
    group_id: i64,
    success: bool,
    status: String,
    request_id: String,
    message_id: String,
    error: String,
}

#[tauri::command]
pub(crate) async fn send_text(
    state: State<'_, AppState>,
    group_id: i64,
    text: String,
) -> AppResult<String> {
    require_manager(&state, group_id).await?;
    let (sender_id, account_id) = state.gateway.session_identity().await?;
    let result = state.gateway.send_text(group_id, &text).await;
    let receipt = archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "send_text",
        0,
        "人工发送群消息",
        result,
    )
    .await?;
    persist_outgoing_message(
        &state,
        &account_id,
        group_id,
        sender_id,
        text.trim(),
        &receipt.message_id,
    )
    .await?;
    Ok(receipt.message_id)
}

#[tauri::command]
pub(crate) async fn send_text_batch(
    state: State<'_, AppState>,
    group_ids: Vec<i64>,
    text: String,
) -> AppResult<Vec<BatchSendResult>> {
    if text.trim().is_empty() {
        return Err(AppError::new("message_empty", "发送内容不能为空"));
    }
    for group_id in &group_ids {
        require_manager(&state, *group_id).await?;
    }
    let (sender_id, account_id) = state.gateway.session_identity().await?;
    let mut results = Vec::with_capacity(group_ids.len());
    for group_id in group_ids {
        let result = state.gateway.send_text(group_id, text.trim()).await;
        match archive_manual_gateway_result(
            &state,
            &account_id,
            group_id,
            0,
            "send_text",
            0,
            "人工批量发送群消息",
            result,
        )
        .await
        {
            Ok(receipt) => {
                let local_error = persist_outgoing_message(
                    &state,
                    &account_id,
                    group_id,
                    sender_id,
                    text.trim(),
                    &receipt.message_id,
                )
                .await
                .err()
                .map(|error| format!("消息已发送，本地记录失败：{}", error.message))
                .unwrap_or_default();
                results.push(BatchSendResult {
                    group_id,
                    success: true,
                    message_id: receipt.message_id,
                    error: local_error,
                });
            }
            Err(error) => results.push(BatchSendResult {
                group_id,
                success: false,
                message_id: String::new(),
                error: error.message,
            }),
        }
    }
    Ok(results)
}

async fn persist_outgoing_message(
    state: &State<'_, AppState>,
    account_id: &str,
    group_id: i64,
    sender_id: i64,
    text: &str,
    server_message_id: &str,
) -> AppResult<()> {
    if server_message_id.trim().is_empty() {
        return Err(AppError::new(
            "sent_message_id",
            "消息已发送，但协议回执缺少服务器消息编号",
        ));
    }
    let now = Utc::now();
    state
        .database_executor
        .insert_message(Message {
            id: 0,
            account_id: account_id.into(),
            group_id,
            server_message_id: server_message_id.into(),
            sequence: 0,
            user_id: sender_id,
            sender_name: "当前账号".into(),
            kind: "text".into(),
            text: text.into(),
            sent_at: now,
            received_at: now,
            processed_at: Some(now),
            acknowledged_at: Some(now),
            processing_state: "processed".into(),
            attempts: 1,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: Some("manual-send".into()),
            flow: Some("out".into()),
        })
        .await
        .map(|_| ())
}

#[tauri::command]
pub(crate) async fn execute_group_batch(
    state: State<'_, AppState>,
    input: GroupBatchInput,
) -> AppResult<Vec<GroupBatchResult>> {
    let (action, group_ids, text) = validate_group_batch_input(input)?;

    let account_id = state.gateway.session_identity().await?.1;
    let (kind, reason) = match action {
        GroupBatchAction::Announcement => ("announcement", "人工批量发布新群公告"),
        GroupBatchAction::Mute => ("group_mute", "人工批量全员禁言"),
        GroupBatchAction::Unmute => ("group_mute", "人工批量解除全禁"),
    };
    let mut results = Vec::with_capacity(group_ids.len());

    for group_id in group_ids {
        let gateway_result = async {
            require_manager(&state, group_id).await?;
            match action {
                GroupBatchAction::Announcement => {
                    state.gateway.set_group_announcement(group_id, &text).await
                }
                GroupBatchAction::Mute => state.gateway.set_group_mute(group_id, true).await,
                GroupBatchAction::Unmute => state.gateway.set_group_mute(group_id, false).await,
            }
        }
        .await;

        match archive_manual_gateway_result(
            &state,
            &account_id,
            group_id,
            0,
            kind,
            0,
            reason,
            gateway_result,
        )
        .await
        {
            Ok(receipt) => results.push(GroupBatchResult {
                group_id,
                success: true,
                status: receipt.status,
                request_id: receipt.request_id,
                message_id: receipt.message_id,
                error: String::new(),
            }),
            Err(error) => results.push(GroupBatchResult {
                group_id,
                success: false,
                status: if error.delivery_outcome_unknown() {
                    "unknown".into()
                } else {
                    "failed".into()
                },
                request_id: String::new(),
                message_id: String::new(),
                error: error.message,
            }),
        }
    }

    Ok(results)
}

#[tauri::command]
pub(crate) async fn query_messages(
    state: State<'_, AppState>,
    mut query: MessageQuery,
) -> AppResult<Page<Message>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    query.limit = Some(limit + 1);
    let mut items = state.database_executor.query_messages(query).await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|message| message.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
pub(crate) async fn recent_messages(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<Message>> {
    state
        .database_executor
        .list_messages(account_id, group_id, limit.unwrap_or(100))
        .await
}
