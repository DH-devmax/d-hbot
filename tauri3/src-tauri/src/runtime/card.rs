//! 名片改名队列与欢迎语。
//!
//! 本文件是 `BackendRuntime` 的一部分；字段与私有辅助函数通过
//! `use super::*` 从 `runtime/mod.rs` 继承。

use super::*;

impl BackendRuntime {
    pub(super) async fn card_queue_loop(&self, app: AppHandle) {
        loop {
            let globally_paused = self
                .database
                .get_setting("automation.mode".into())
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some("paused");
            if !globally_paused && self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((sender_id, account_id)) = self.gateway.session_identity().await {
                    if let Ok(_permit) = self.gateway.automatic_write_permit().await {
                        if let Ok(Some(job)) =
                            self.database.claim_next_card_job(account_id.clone()).await
                        {
                            let work_id =
                                runtime_lane_id("cardRename", &job.account_id, job.group_id);
                            self.coordination.tracker.start_named(
                                &work_id,
                                "cardRename",
                                "批量修改群名片",
                                "群名片队列",
                                Some(3),
                            );
                            self.coordination.tracker.progress(&work_id, 1, 3);
                            let paused = self
                                .database
                                .get_setting(format!(
                                    "card.paused.{}.{}",
                                    job.account_id, job.group_id
                                ))
                                .await
                                .ok()
                                .flatten()
                                .as_deref()
                                == Some("true");
                            let result = if paused {
                                Err(AppError::new("card_queue_paused", "群名片队列已暂停"))
                            } else if !self.is_group_manager(sender_id, job.group_id).await {
                                Err(AppError::new(
                                    "management_required",
                                    "需要将账号权限设置为管理",
                                ))
                            } else {
                                self.gateway
                                    .rename(
                                        job.group_id,
                                        &MemberRef {
                                            user_id: Some(job.user_id),
                                            nim_id: (!job.nim_id.is_empty())
                                                .then(|| job.nim_id.clone()),
                                        },
                                        &job.desired_name,
                                    )
                                    .await
                            };
                            let result = match result {
                                Ok(receipt) => {
                                    self.coordination.tracker.progress(&work_id, 2, 3);
                                    sleep(Duration::from_millis(500)).await;
                                    match self.gateway.list_members(job.group_id).await {
                                        Ok(roster) => {
                                            if roster.members.iter().any(|member| {
                                                (member.user_id == job.user_id
                                                    || (!job.nim_id.is_empty()
                                                        && member.nim_id == job.nim_id))
                                                    && member.card_name == job.desired_name
                                            }) {
                                                Ok(receipt)
                                            } else {
                                                Err(AppError::new(
                                                    "card_verify",
                                                    "群名片修改回执成功，但重新读取后尚未生效",
                                                )
                                                .retryable())
                                            }
                                        }
                                        Err(error) => Err(error),
                                    }
                                }
                                Err(error) => Err(error),
                            };
                            let (success, retryable, error, audit_details) = match result {
                                Ok(receipt) => (
                                    true,
                                    false,
                                    String::new(),
                                    serde_json::json!({
                                        "jobId": job.id,
                                        "originalName": job.original_name,
                                        "desiredName": job.desired_name,
                                        "attempts": job.attempts,
                                        "status": "succeeded",
                                        "verification": "verified",
                                        "receipt": redact_gateway_receipt(receipt),
                                    })
                                    .to_string(),
                                ),
                                Err(operation_error) => {
                                    let error_message = redact(&operation_error.message);
                                    let gateway_error = operation_error.gateway.as_deref().cloned();
                                    (
                                        false,
                                        operation_error.retryable
                                            && !operation_error.delivery_outcome_unknown(),
                                        error_message.clone(),
                                        serde_json::json!({
                                            "jobId": job.id,
                                            "originalName": job.original_name,
                                            "desiredName": job.desired_name,
                                            "attempts": job.attempts,
                                            "status": if operation_error.delivery_outcome_unknown() { "unknown" } else { "failed" },
                                            "verification": if operation_error.code == "card_verify" { "failed" } else { "not-applicable" },
                                            "errorCode": operation_error.code,
                                            "error": error_message,
                                            "retryable": operation_error.retryable,
                                            "errorKind": gateway_error.as_ref().map(|value| format!("{:?}", value.kind)),
                                            "route": gateway_error.as_ref().map(|value| value.route.clone()),
                                            "transportCode": gateway_error.as_ref().and_then(|value| value.transport_code),
                                            "transportErrno": gateway_error.as_ref().and_then(|value| value.transport_errno),
                                            "businessCode": gateway_error.as_ref().and_then(|value| value.business_code),
                                            "businessErrno": gateway_error.as_ref().and_then(|value| value.business_errno),
                                        }).to_string(),
                                    )
                                }
                            };
                            let _ = self
                                .database
                                .finish_card_job(job.clone(), success, retryable, error.clone())
                                .await;
                            if success {
                                self.coordination.tracker.progress(&work_id, 3, 3);
                                self.coordination.tracker.finish(&work_id, "succeeded", "");
                            } else if retryable && job.attempts < 5 {
                                self.coordination.tracker.retrying(&work_id, None, &error);
                            } else {
                                self.coordination.tracker.finish(&work_id, "failed", &error);
                            }
                            if success && job.welcome_pending {
                                let welcome = self
                                    .database
                                    .list_groups(Some(job.account_id.clone()))
                                    .await
                                    .ok()
                                    .and_then(|groups| {
                                        groups
                                            .into_iter()
                                            .find(|group| group.group_id == job.group_id)
                                    })
                                    .map(|group| group.welcome_message)
                                    .unwrap_or_default();
                                if welcome.trim().is_empty() {
                                    let _ = self.database.mark_card_welcome_sent(job.id).await;
                                } else {
                                    let _ = self.enqueue_text_effect(
                                    &job.account_id,
                                    job.group_id,
                                    &welcome.replace("[成员]", &job.desired_name),
                                    "member-welcome",
                                    format!("card-welcome:{}", job.id),
                                    serde_json::json!({"cardJobId":job.id,"userId":job.user_id}),
                                ).await;
                                }
                            }
                            let _ = self
                                .database
                                .record_audit(AuditEvent {
                                    id: 0,
                                    account_id: job.account_id.clone(),
                                    group_id: job.group_id,
                                    user_id: job.user_id,
                                    actor: "DH BOT".into(),
                                    event: "card_rename_job".into(),
                                    level: if success { "success" } else { "error" }.into(),
                                    details: audit_details,
                                    created_at: Utc::now(),
                                })
                                .await;
                            let _ = app.emit(
                            "task-progress",
                            serde_json::json!({
                                "kind":"cardRename",
                                "groupId":job.group_id,
                                "userId":job.user_id,
                                "state":if success { "succeeded" } else if !retryable || job.attempts >= 5 { "failed" } else { "retry" },
                                "error":error
                            }),
                        );
                            tokio::select! {
                                _ = sleep(Duration::from_millis(500)) => {},
                                _ = self.shutdown.cancelled() => break,
                            }
                            continue;
                        }
                    }
                }
            }
            tokio::select! {
                _ = self.coordination.wake.notified() => {},
                _ = sleep(Duration::from_secs(30)) => {},
                _ = self.shutdown.cancelled() => break,
            }
        }
    }

    pub(super) async fn enqueue_discovered_member_card_jobs(
        &self,
        account_id: &str,
        group_id: i64,
        member_ids: &HashSet<i64>,
    ) -> AppResult<()> {
        if self
            .database
            .get_setting(format!("card.auto.{account_id}.{group_id}"))
            .await?
            .as_deref()
            != Some("true")
        {
            return Ok(());
        }
        let prefix = self
            .database
            .get_setting(format!("card.prefix.{account_id}.{group_id}"))
            .await?
            .unwrap_or_else(|| "DH".into());
        let members = self
            .database
            .list_members(account_id.to_string(), group_id)
            .await?
            .into_iter()
            .filter(|member| member.present)
            .collect();
        let sender_id = self.gateway.session_identity().await?.0;
        let preview = crate::cardnames::preview(group_id, &prefix, members, sender_id)?;
        for plan in preview
            .items
            .into_iter()
            .filter(|plan| plan.status == "planned" && member_ids.contains(&plan.member.user_id))
        {
            if self
                .database
                .enqueue_card_job(account_id.to_string(), group_id, plan, true)
                .await?
            {
                let work_id = runtime_lane_id("cardRename", account_id, group_id);
                self.coordination.tracker.enqueue(
                    &work_id,
                    "cardRename",
                    "批量修改群名片",
                    "群名片队列",
                );
                self.coordination.notify();
            }
        }
        Ok(())
    }

    pub(super) async fn enqueue_member_card_job(
        &self,
        account_id: &str,
        group_id: i64,
        member: &Member,
    ) -> AppResult<()> {
        if self
            .database
            .get_setting(format!("card.auto.{account_id}.{group_id}"))
            .await?
            .as_deref()
            != Some("true")
        {
            return Ok(());
        }
        let prefix = self
            .database
            .get_setting(format!("card.prefix.{account_id}.{group_id}"))
            .await?
            .unwrap_or_else(|| "DH".into());
        let members = self
            .database
            .list_members(account_id.to_string(), group_id)
            .await?;
        let sender_id = self.gateway.session_identity().await?.0;
        let preview = crate::cardnames::preview(group_id, &prefix, members, sender_id)?;
        if let Some(plan) = preview
            .items
            .into_iter()
            .find(|plan| plan.member.user_id == member.user_id && plan.status == "planned")
        {
            if self
                .database
                .enqueue_card_job(account_id.to_string(), group_id, plan, true)
                .await?
            {
                let work_id = runtime_lane_id("cardRename", account_id, group_id);
                self.coordination.tracker.enqueue(
                    &work_id,
                    "cardRename",
                    "批量修改群名片",
                    "群名片队列",
                );
                self.coordination.notify();
            }
        }
        Ok(())
    }
}
