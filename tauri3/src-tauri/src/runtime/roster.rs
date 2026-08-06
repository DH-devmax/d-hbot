//! 成员名单对账与生命周期。
//!
//! 本文件是 `BackendRuntime` 的一部分；字段与私有辅助函数通过
//! `use super::*` 从 `runtime/mod.rs` 继承。

use super::*;

impl BackendRuntime {
    pub(super) async fn roster_sync_loop(&self) {
        let mut delay = Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = sleep(delay) => {},
                _ = self.shutdown.cancelled() => break,
            }
            if !self.gateway.calibration_active()
                && self.gateway.diagnose().await.status == ConnectionStatus::Ready
            {
                if let Ok((_, account_id)) = self.gateway.session_identity().await {
                    self.sync_members(&account_id).await;
                }
            }
            delay = Duration::from_secs(60);
        }
    }

    pub(super) async fn sync_members(&self, account_id: &str) {
        let session_epoch = self.gateway.session_epoch();
        if self.gateway.calibration_active() || self.gateway.member_sync_paused() {
            return;
        }
        let groups = match self.gateway.list_groups().await {
            Ok(groups) => groups,
            Err(error) => {
                self.logger.write(
                    "WARN",
                    &format!("群成员对账无法读取群列表：{}", error.message),
                );
                return;
            }
        };
        let enabled_group_ids = self
            .database
            .list_groups(Some(account_id.to_string()))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|group| group.enabled)
            .map(|group| group.group_id)
            .collect::<HashSet<_>>();
        let mut synced_enabled_group = false;
        for group in groups {
            if self.gateway.calibration_active()
                || self.gateway.session_epoch() != session_epoch
                || self.gateway.member_sync_paused()
            {
                return;
            }
            let _ = self.database.upsert_group(group.clone()).await;
            if !enabled_group_ids.contains(&group.group_id) {
                continue;
            }
            if synced_enabled_group {
                tokio::select! {
                    _ = sleep(Duration::from_secs(2)) => {},
                    _ = self.shutdown.cancelled() => return,
                }
            }
            synced_enabled_group = true;
            self.gateway.invalidate_member_cache(group.group_id).await;
            let snapshot_started_at = Utc::now();
            let roster = match self.gateway.list_members(group.group_id).await {
                Ok(roster) => roster,
                Err(error) => {
                    self.events.emit(
                        "member-roster-status",
                        serde_json::json!({
                            "accountId": account_id,
                            "groupId": group.group_id,
                            "status": if error.code == "gateway_rate_limited" { "rate-limited" } else { "error" },
                            "reportedCount": 0,
                            "resolvedCount": 0,
                            "complete": false,
                            "completenessReason": error.message,
                        }),
                    );
                    return;
                }
            };
            if roster.authority != "authoritative"
                || !roster.source_errors.is_empty()
                || self.gateway.session_epoch() != session_epoch
            {
                continue;
            }
            let existing = self
                .database
                .list_members(account_id.to_string(), group.group_id)
                .await
                .unwrap_or_default();
            let baseline = existing.is_empty();
            let mut present_ids = HashSet::new();
            let mut newly_discovered = HashSet::new();
            let mut card_candidate_ids = HashSet::new();
            let mut invalid_card_ids = HashSet::new();
            for mut member in roster.members {
                member.account_id = account_id.to_string();
                let mut managed_name_drifted = false;
                let mut drift_audit = None;
                let is_new = if let Some(saved) = existing.iter().find(|saved| {
                    saved.user_id == member.user_id
                        || (!member.nim_id.is_empty() && saved.nim_id == member.nim_id)
                }) {
                    merge_managed_member_state(&mut member, saved);
                    let locked_name = if !saved.locked_card_name.trim().is_empty() {
                        saved.locked_card_name.trim()
                    } else {
                        saved.managed_card_name.trim()
                    };
                    managed_name_drifted =
                        !locked_name.is_empty() && member.card_name.trim() != locked_name;
                    if managed_name_drifted && saved.card_name.trim() != member.card_name.trim() {
                        drift_audit = Some((member.card_name.clone(), locked_name.to_string()));
                    }
                    if !saved.present {
                        let now = Utc::now();
                        member.join_source = "offline-discovered".into();
                        member.joined_at = Some(now);
                        true
                    } else {
                        false
                    }
                } else {
                    let now = Utc::now();
                    member.original_card_name = member.card_name.clone();
                    member.join_source = if baseline {
                        "baseline"
                    } else {
                        "offline-discovered"
                    }
                    .into();
                    member.prompt_read = baseline;
                    member.discovered_at = now;
                    member.joined_at = (!baseline).then_some(now);
                    true
                };
                member.present = true;
                member.last_seen_at = Utc::now();
                member.updated_at = member.last_seen_at;
                let needs_card_correction =
                    crate::cardnames::automatic_correction_eligible(&member);
                if let Ok(canonical_user_id) = self.database.upsert_member(member.clone()).await {
                    present_ids.insert(canonical_user_id);
                    if let Some((observed, locked)) = drift_audit {
                        let count = self
                            .database
                            .increment_member_violation(
                                account_id.to_string(),
                                group.group_id,
                                canonical_user_id,
                            )
                            .await
                            .unwrap_or(member.violation_count);
                        let _ = self
                            .database
                            .record_audit(AuditEvent {
                                id: 0,
                                account_id: account_id.into(),
                                group_id: group.group_id,
                                user_id: canonical_user_id,
                                actor: "DH BOT".into(),
                                event: "locked_card_drift_detected".into(),
                                level: "warning".into(),
                                details: serde_json::json!({
                                    "source": "authoritative-roster",
                                    "observedName": observed,
                                    "lockedName": locked,
                                    "violationCount": count,
                                    "action": "queued-for-restore",
                                })
                                .to_string(),
                                created_at: Utc::now(),
                            })
                            .await;
                    }
                    if is_new {
                        newly_discovered.insert(canonical_user_id);
                        if !member.blacklisted {
                            card_candidate_ids.insert(canonical_user_id);
                        }
                    } else if managed_name_drifted && !member.blacklisted {
                        card_candidate_ids.insert(canonical_user_id);
                    }
                    if !baseline && needs_card_correction && !member.blacklisted {
                        invalid_card_ids.insert(canonical_user_id);
                        card_candidate_ids.insert(canonical_user_id);
                    }
                }
                if !baseline && is_new && member.blacklisted {
                    let queued = self
                        .enqueue_effect(
                            account_id,
                            group.group_id,
                            "remove",
                            serde_json::json!({
                                "userId": member.user_id,
                                "recordAction": true,
                                "actionKind": "remove",
                                "mode": "automatic",
                                "reason": "blacklisted_member_rejoined",
                            }),
                            format!(
                                "blacklist-roster-rejoin:{account_id}:{}:{}",
                                group.group_id, member.user_id
                            ),
                        )
                        .await
                        .is_ok();
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id: group.group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "blacklisted_member_rejoined".into(),
                            level: if queued { "info" } else { "error" }.into(),
                            details: "名单对账发现黑名单成员重新入群".into(),
                            created_at: Utc::now(),
                        })
                        .await;
                }
            }
            if !baseline && !card_candidate_ids.is_empty() {
                if let Err(error) = self
                    .enqueue_discovered_member_card_jobs(
                        account_id,
                        group.group_id,
                        &card_candidate_ids,
                    )
                    .await
                {
                    self.logger.write(
                        "WARN",
                        &format!("成员群名片自动校正入队失败：{}", error.message),
                    );
                }
                if !invalid_card_ids.is_empty() {
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id: group.group_id,
                            user_id: 0,
                            actor: "DH BOT".into(),
                            event: "card_name_correction_queued".into(),
                            level: "warning".into(),
                            details: format!(
                                "名单对账发现 {} 名群员的当前群名片异常，已进入自动纠正队列",
                                invalid_card_ids.len()
                            ),
                            created_at: Utc::now(),
                        })
                        .await;
                }
            }
            if self.gateway.session_epoch() != session_epoch {
                return;
            }
            let missing = self
                .database
                .mark_members_not_present_before(
                    account_id.to_string(),
                    group.group_id,
                    present_ids.into_iter().collect(),
                    snapshot_started_at,
                )
                .await
                .unwrap_or_default();
            let missing_count = missing.len();
            for member in missing {
                let _ = self
                    .database
                    .record_audit(AuditEvent {
                        id: 0,
                        account_id: account_id.into(),
                        group_id: group.group_id,
                        user_id: member.user_id,
                        actor: "DH BOT".into(),
                        event: "member_left".into(),
                        level: "info".into(),
                        details: "名单对账确认成员已离群".into(),
                        created_at: Utc::now(),
                    })
                    .await;
            }
            self.events.emit(
                "member-roster-status",
                serde_json::json!({
                    "accountId": account_id,
                    "groupId": group.group_id,
                    "status": roster.status,
                    "reportedCount": roster.reported_count,
                    "resolvedCount": roster.resolved_count,
                    "complete": roster.complete,
                    "source": "background-reconciliation",
                    "newlyDiscovered": newly_discovered.len(),
                    "missing": missing_count,
                }),
            );
            if self.gateway.member_sync_paused() {
                return;
            }
        }
    }
}
