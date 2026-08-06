//! 审计事件查询与导出。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) async fn list_audit(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<AuditEvent>> {
    state
        .database_executor
        .list_audit(account_id, limit.unwrap_or(200))
        .await
}

#[tauri::command]
pub(crate) async fn query_audit(
    state: State<'_, AppState>,
    mut query: AuditQuery,
) -> AppResult<Page<AuditEvent>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    query.limit = Some(limit + 1);
    let mut items = state.database_executor.query_audit(query).await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|item| item.id.to_string())
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

#[tauri::command]
pub(crate) async fn export_audit(
    state: State<'_, AppState>,
    mut query: AuditQuery,
    format: String,
) -> AppResult<String> {
    query.cursor = None;
    query.limit = Some(1000);
    let events = state.database_executor.query_audit(query).await?;
    if format.eq_ignore_ascii_case("json") {
        let localized = events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "时间": event.created_at.to_rfc3339(),
                    "账号": event.account_id,
                    "群ID": event.group_id,
                    "成员ID": event.user_id,
                    "执行者": event.actor,
                    "事件": audit_event_label(&event.event),
                    "级别": audit_level_label(&event.level),
                    "详情": localize_audit_details(&event.details),
                })
            })
            .collect::<Vec<_>>();
        return serde_json::to_string_pretty(&localized)
            .map_err(|error| AppError::new("audit_export", error.to_string()));
    }
    let mut output = String::from("时间,账号,群ID,成员ID,执行者,事件,级别,详情\n");
    for event in events {
        let cells = [
            event.created_at.to_rfc3339(),
            event.account_id,
            event.group_id.to_string(),
            event.user_id.to_string(),
            event.actor,
            audit_event_label(&event.event).to_string(),
            audit_level_label(&event.level).to_string(),
            localize_audit_details(&event.details),
        ];
        output.push_str(
            &cells
                .into_iter()
                .map(|value| format!("\"{}\"", value.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push('\n');
    }
    Ok(output)
}

fn audit_level_label(value: &str) -> &str {
    match value {
        "direction" => "消息方向",
        "kind" => "消息类型",
        "sequence" => "接收队列序号",
        "serverMessageId" => "旺商聊消息编号",
        "senderName" => "发送成员名称",
        "contentPreview" => "内容摘要",
        "processingState" => "处理状态",
        "source" => "消息来源",
        "flow" => "消息流向",
        "result" => "处理结果",
        "inserted" => "首次入库",
        "duplicate" => "是否重复",
        "expected" => "预期事件数",
        "normalized" => "成功识别数",
        "fallback" => "后续处理",
        "jobId" => "任务编号",
        "originalName" => "原群名片",
        "desiredName" => "目标群名片",
        "attempts" => "当前尝试次数",
        "errorCode" => "错误代码",
        "errorKind" => "错误类型",
        "info" => "信息",
        "warning" => "警告",
        "error" => "错误",
        "success" => "成功",
        _ => value,
    }
}
