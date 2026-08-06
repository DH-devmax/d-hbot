//! 撤回、禁言、改名、移出与群公告。

use tauri::State;

use crate::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupManagementContext {
    group_id: i64,
    sender_id: i64,
    is_manager: bool,
    capabilities: gateway::GatewayCapabilities,
    member_count: usize,
    announcement_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberBatchInput {
    group_id: i64,
    action: String,
    members: Vec<MemberRef>,
    duration_seconds: Option<i64>,
    nickname: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberBatchResult {
    user_id: Option<i64>,
    nim_id: Option<String>,
    success: bool,
    error: String,
}

#[tauri::command]
pub(crate) async fn recall_message(
    state: State<'_, AppState>,
    group_id: i64,
    sender_user_id: i64,
    message_id: String,
) -> AppResult<()> {
    require_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .recall(group_id, sender_user_id, &message_id)
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        sender_user_id,
        "recall",
        0,
        "人工撤回消息",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn mute_member(
    state: State<'_, AppState>,
    group_id: i64,
    user_id: i64,
    duration_seconds: i64,
) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .mute(group_id, target.user_id, duration_seconds)
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "mute",
        duration_seconds,
        "人工禁言成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn unmute_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.unmute(group_id, target.user_id).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "unmute",
        0,
        "人工解禁成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn rename_member(
    state: State<'_, AppState>,
    group_id: i64,
    member: MemberRef,
    nickname: String,
) -> AppResult<()> {
    let nickname = cardnames::validate_card_name(&nickname)?;
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member_ref(&roster, &member)?;
    let target_ref = MemberRef {
        user_id: Some(target.user_id),
        nim_id: (!target.nim_id.is_empty()).then(|| target.nim_id.clone()),
    };
    let account_id = state.gateway.session_identity().await?.1;
    state
        .database_executor
        .expect_member_card_update(
            account_id.clone(),
            group_id,
            target.user_id,
            nickname.clone(),
            format!("manual-rename:{}:{}", group_id, target.user_id),
        )
        .await?;
    let result = state.gateway.rename(group_id, &target_ref, &nickname).await;
    let rename_failed = result
        .as_ref()
        .map(|receipt| receipt.status == "failed")
        .unwrap_or(true);
    if rename_failed {
        let _ = state
            .database_executor
            .consume_expected_member_card_update(
                account_id.clone(),
                group_id,
                target.user_id,
                nickname.clone(),
            )
            .await;
    }
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        target.user_id,
        "rename",
        0,
        "人工修改群名片",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn remove_member(state: State<'_, AppState>, group_id: i64, user_id: i64) -> AppResult<()> {
    let roster = require_member_manager(&state, group_id).await?;
    let target = canonical_member(&roster, user_id)?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.remove_member(group_id, target.user_id).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        user_id,
        "remove",
        0,
        "人工移出成员",
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn set_group_mute(state: State<'_, AppState>, group_id: i64, muted: bool) -> AppResult<()> {
    let _roster = require_member_manager(&state, group_id).await?;
    let account_id = state.gateway.session_identity().await?.1;
    let result = state.gateway.set_group_mute(group_id, muted).await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "group_mute",
        0,
        if muted {
            "人工关群"
        } else {
            "人工开群"
        },
        result,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) async fn get_group_mute_state(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<GroupMuteState> {
    state.gateway.get_group_mute_state(group_id).await
}

#[tauri::command]
pub(crate) async fn set_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
    text: String,
) -> AppResult<GatewayReceipt> {
    let _roster = require_member_manager(&state, group_id).await?;
    if text.trim().is_empty() {
        return Err(AppError::new("announcement_empty", "群公告内容不能为空"));
    }
    let account_id = state.gateway.session_identity().await?.1;
    let result = state
        .gateway
        .set_group_announcement(group_id, text.trim())
        .await;
    let receipt = archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "announcement",
        0,
        "人工发布新群公告",
        result,
    )
    .await?;
    if receipt.verification.as_deref() == Some("verified") {
        let capability = state
            .gateway
            .mark_capability_verified("announcement")
            .announcement;
        let _ = state
            .database_executor
            .save_gateway_capability_verification(
                capability.fingerprint.clone(),
                "announcement".into(),
                "manualReceipt".into(),
                "supported".into(),
                true,
                capability.fingerprint,
                String::new(),
            )
            .await;
    }
    Ok(receipt)
}

#[tauri::command]
pub(crate) async fn get_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<Option<GroupAnnouncement>> {
    state.gateway.get_group_announcement(group_id).await
}

#[tauri::command]
pub(crate) async fn list_group_announcements(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<Vec<GroupAnnouncement>> {
    state.gateway.list_group_announcements(group_id).await
}

#[tauri::command]
pub(crate) async fn update_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
    notice_id: String,
    text: String,
    mode: String,
) -> AppResult<GatewayReceipt> {
    let _roster = require_member_manager(&state, group_id).await?;
    if notice_id.trim().is_empty() || text.trim().is_empty() || text.chars().count() > 1000 {
        return Err(AppError::new(
            "announcement_invalid",
            "请选择有效公告，并将内容控制在 1 到 1000 个字符",
        ));
    }
    let (sender_id, account_id) = state.gateway.session_identity().await?;
    let existing = state
        .gateway
        .list_group_announcements(group_id)
        .await?
        .into_iter()
        .find(|announcement| announcement.notice_id == notice_id)
        .ok_or_else(|| AppError::new("announcement_not_found", "公告历史中没有找到该公告"))?;
    if existing.author_user_id > 0 && existing.author_user_id != sender_id {
        return Err(AppError::new(
            "announcement_author",
            "旺商聊只允许公告作者编辑这条公告",
        ));
    }
    let result = state
        .gateway
        .update_group_announcement(group_id, notice_id.trim(), text.trim(), mode.trim())
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "announcement_update",
        0,
        if mode == "TOP_NOTICE" {
            "人工编辑并置顶群公告"
        } else {
            "人工编辑群公告"
        },
        result,
    )
    .await
}

#[tauri::command]
pub(crate) async fn delete_group_announcement(
    state: State<'_, AppState>,
    group_id: i64,
    notice_id: String,
) -> AppResult<GatewayReceipt> {
    let _roster = require_member_manager(&state, group_id).await?;
    if notice_id.trim().is_empty() {
        return Err(AppError::new("announcement_invalid", "请选择要删除的公告"));
    }
    let (sender_id, account_id) = state.gateway.session_identity().await?;
    let existing = state
        .gateway
        .list_group_announcements(group_id)
        .await?
        .into_iter()
        .find(|announcement| announcement.notice_id == notice_id)
        .ok_or_else(|| AppError::new("announcement_not_found", "公告历史中没有找到该公告"))?;
    if existing.author_user_id > 0 && existing.author_user_id != sender_id {
        return Err(AppError::new(
            "announcement_author",
            "旺商聊只允许公告作者删除这条公告",
        ));
    }
    let result = state
        .gateway
        .delete_group_announcement(group_id, notice_id.trim())
        .await;
    archive_manual_gateway_result(
        &state,
        &account_id,
        group_id,
        0,
        "announcement_delete",
        0,
        "人工删除群公告",
        result,
    )
    .await
}

#[tauri::command]
pub(crate) async fn get_group_management_context(
    state: State<'_, AppState>,
    group_id: i64,
) -> AppResult<GroupManagementContext> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    let is_manager = roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    });
    let capabilities = state.gateway.capabilities();
    #[cfg(feature = "fixture")]
    let calibration_allows_announcement = {
        let calibration = state.gateway.developer_calibration_status();
        calibration.active
            && !calibration.finishing
            && calibration
                .capabilities
                .iter()
                .any(|capability| capability == "announcement")
    };
    #[cfg(not(feature = "fixture"))]
    let calibration_allows_announcement = false;
    let announcement_status = if calibration_allows_announcement {
        "可用"
    } else {
        match capabilities.announcement.status {
            gateway::CapabilityStatus::Supported => "可用",
            gateway::CapabilityStatus::ManualVerification => "待首次手工验证",
            gateway::CapabilityStatus::Unavailable => "当前协议结构不可用",
            gateway::CapabilityStatus::Unsupported => "功能未开放",
        }
    };
    Ok(GroupManagementContext {
        group_id,
        sender_id,
        is_manager,
        capabilities,
        member_count: roster.members.len(),
        announcement_status: announcement_status.into(),
    })
}

#[tauri::command]
pub(crate) async fn execute_member_batch(
    state: State<'_, AppState>,
    input: MemberBatchInput,
) -> AppResult<Vec<MemberBatchResult>> {
    let roster = require_member_manager(&state, input.group_id).await?;
    let action = input.action.trim().to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "mute" | "unmute" | "remove" | "blacklist" | "unblacklist" | "rename"
    ) {
        return Err(AppError::new("batch_action", "成员批量动作不受支持"));
    }
    if action == "rename" && input.nickname.as_deref().unwrap_or("").trim().is_empty() {
        return Err(AppError::new("batch_action", "批量改名需要填写名称"));
    }
    let account_id = state.gateway.session_identity().await?.1;
    let mut results = Vec::with_capacity(input.members.len());
    for member in input.members {
        let canonical = canonical_member_ref(&roster, &member)?;
        let user_id = canonical.user_id;
        let canonical_ref = MemberRef {
            user_id: Some(canonical.user_id),
            nim_id: (!canonical.nim_id.is_empty()).then(|| canonical.nim_id.clone()),
        };
        let nim_id = member.nim_id.clone();
        let result = if user_id <= 0 && action != "rename" {
            Err(AppError::new("member_identity", "该成员尚未解析出旺商号"))
        } else {
            match action.as_str() {
                "mute" => {
                    state
                        .gateway
                        .mute(
                            input.group_id,
                            user_id,
                            input.duration_seconds.unwrap_or(600),
                        )
                        .await
                }
                "unmute" => state.gateway.unmute(input.group_id, user_id).await,
                "remove" => state.gateway.remove_member(input.group_id, user_id).await,
                "rename" => {
                    state
                        .gateway
                        .rename(
                            input.group_id,
                            &canonical_ref,
                            input.nickname.as_deref().unwrap_or("").trim(),
                        )
                        .await
                }
                "blacklist" => state
                    .database_executor
                    .set_member_blacklisted(account_id.clone(), input.group_id, user_id, true)
                    .await
                    .map(|_| GatewayReceipt::succeeded("database.member.blacklist")),
                "unblacklist" => state
                    .database_executor
                    .set_member_blacklisted(account_id.clone(), input.group_id, user_id, false)
                    .await
                    .map(|_| GatewayReceipt::succeeded("database.member.unblacklist")),
                _ => unreachable!(),
            }
        };
        let archived = archive_manual_gateway_result(
            &state,
            &account_id,
            input.group_id,
            user_id,
            &action,
            input.duration_seconds.unwrap_or(0),
            "人工批量成员操作",
            result,
        )
        .await;
        results.push(MemberBatchResult {
            user_id: member.user_id,
            nim_id,
            success: archived.is_ok(),
            error: archived
                .err()
                .map(|error| error.message)
                .unwrap_or_default(),
        });
    }
    Ok(results)
}
