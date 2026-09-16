//! 群列表、成员名单与群级开关。

#[cfg(feature = "headless")]
use crate::headless::State;
#[cfg(not(feature = "headless"))]
use tauri::State;

use crate::*;

pub(crate) fn validate_roster_context(
    members: &[Member],
    account: &str,
    group: i64,
    requested_epoch: u64,
    current_epoch: u64,
) -> AppResult<()> {
    if requested_epoch != current_epoch
        || members
            .iter()
            .any(|member| member.account_id != account || member.group_id != group)
    {
        return Err(AppError::new(
            "stale_roster",
            "账号或群状态已变化，请重新读取成员名单",
        ));
    }
    Ok(())
}

#[cfg_attr(not(feature = "headless"), tauri::command)]
pub(crate) async fn list_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    crate::service::BusinessService {
        database: state.database_executor.clone(),
        gateway: state.gateway.clone(),
    }
    .list_groups()
    .await
}

#[cfg_attr(not(feature = "headless"), tauri::command)]
pub(crate) async fn list_cached_groups(state: State<'_, AppState>) -> AppResult<Vec<Group>> {
    let Ok((_, account)) = state.gateway.session_identity().await else { return Ok(Vec::new()); };
    state.database_executor.list_groups(Some(account)).await
}

#[cfg_attr(not(feature = "headless"), tauri::command)]
pub(crate) async fn list_members(
    app: crate::CommandEventHandle,
    state: State<'_, AppState>,
    group_id: i64,
    refresh: Option<bool>,
) -> AppResult<MemberRoster> {
    let request_epoch = state.gateway.session_epoch();
    let snapshot_started_at = Utc::now();
    if refresh.unwrap_or(false) {
        state.gateway.invalidate_member_cache(group_id).await;
    }
    let mut roster = state.gateway.list_members(group_id).await?;
    let (self_id, account_id) = state.gateway.session_identity().await?;
    validate_roster_context(
        &roster.members,
        &account_id,
        group_id,
        request_epoch,
        state.gateway.session_epoch(),
    )?;
    let existing = state
        .database_executor
        .list_members(account_id.clone(), group_id)
        .await?;
    let had_baseline = !existing.is_empty();
    let mut newly_discovered = std::collections::HashSet::new();
    let mut present_user_ids = std::collections::HashSet::new();
    for member in &mut roster.members {
        if let Some(saved) = existing.iter().find(|value| {
            value.user_id == member.user_id
                || (!member.nim_id.is_empty() && value.nim_id == member.nim_id)
        }) {
            member.original_card_name = saved.original_card_name.clone();
            member.managed_card_name = saved.managed_card_name.clone();
            member.card_suffix = saved.card_suffix.clone();
            member.blacklisted = saved.blacklisted;
        } else {
            member.original_card_name = member.card_name.clone();
            newly_discovered.insert(member.user_id);
        }
        let canonical_user_id = state
            .database_executor
            .upsert_member(member.clone())
            .await?;
        present_user_ids.insert(canonical_user_id);
    }
    if refresh.unwrap_or(false)
        && roster.authority == "authoritative"
        && request_epoch == state.gateway.session_epoch()
    {
        let _ = state
            .database_executor
            .mark_members_not_present_before(
                account_id.clone(),
                group_id,
                present_user_ids.into_iter().collect(),
                snapshot_started_at,
            )
            .await?;
    }
    let automatic = state
        .database_executor
        .get_setting(format!("card.auto.{account_id}.{group_id}"))
        .await?
        .as_deref()
        == Some("true");
    if had_baseline
        && automatic
        && !newly_discovered.is_empty()
        && request_epoch == state.gateway.session_epoch()
    {
        let prefix = state
            .database_executor
            .get_setting(format!("card.prefix.{account_id}.{group_id}"))
            .await?
            .unwrap_or_else(|| "DH".into());
        let preview_members = state
            .database_executor
            .list_members(account_id.clone(), group_id)
            .await?
            .into_iter()
            .filter(|member| member.present)
            .collect();
        let preview = cardnames::preview(group_id, &prefix, preview_members, self_id)?;
        for plan in preview.items.into_iter().filter(|plan| {
            plan.status == "planned" && newly_discovered.contains(&plan.member.user_id)
        }) {
            if state
                .database_executor
                .enqueue_card_job(account_id.clone(), group_id, plan, true)
                .await?
            {
                let work_id = runtime::runtime_lane_id("cardRename", &account_id, group_id);
                state.runtime_coordination.tracker.enqueue(
                    &work_id,
                    "cardRename",
                    "批量修改群名片",
                    "群名片队列",
                );
                state.runtime_coordination.notify();
            }
        }
    }
    let _ = app.emit(
        "member-roster-status",
        serde_json::json!({
            "accountId": account_id,
            "groupId": group_id,
            "status": roster.status.clone(),
            "reportedCount": roster.reported_count,
            "resolvedCount": roster.resolved_count,
            "complete": roster.complete,
            "completenessReason": roster.completeness_reason.clone(),
            "retryAt": roster.retry_at.clone(),
            "canonicalCount": roster.canonical_count,
            "syntheticUserIds": roster.synthetic_user_ids.clone(),
            "sourceErrors": roster.source_errors.clone(),
        }),
    );
    Ok(roster)
}

#[cfg_attr(not(feature = "headless"), tauri::command)]
pub(crate) async fn local_members(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<Vec<Member>> {
    let members = state
        .database_executor
        .list_members(account_id, group_id)
        .await?;
    Ok(members
        .into_iter()
        .filter(|member| member.present)
        .collect())
}

#[cfg_attr(not(feature = "headless"), tauri::command)]
pub(crate) async fn set_group_features(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    enabled: bool,
    ai_enabled: bool,
    moderation_enabled: bool,
    manual_takeover: bool,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    require_manager(&state, group_id).await?;
    state
        .database_executor
        .set_group_features(
            account_id,
            group_id,
            enabled,
            ai_enabled,
            moderation_enabled,
            manual_takeover,
        )
        .await
}
