//! 群日报生成与定时循环。
//!
//! 本文件是 `BackendRuntime` 的一部分；字段与私有辅助函数通过
//! `use super::*` 从 `runtime/mod.rs` 继承。

use super::*;

impl BackendRuntime {
    pub(super) async fn summary_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((_, account_id)) = self.gateway.session_identity().await {
                    let enabled = self
                        .database
                        .get_setting(format!("summary.enabled.{account_id}"))
                        .await
                        .ok()
                        .flatten()
                        .as_deref()
                        == Some("true");
                    let time = self
                        .database
                        .get_setting(format!("summary.time.{account_id}"))
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "23:00".into());
                    let groups: Vec<i64> = self
                        .database
                        .get_setting(format!("summary.groups.{account_id}"))
                        .await
                        .ok()
                        .flatten()
                        .and_then(|value| serde_json::from_str(&value).ok())
                        .unwrap_or_default();
                    let now = self.clock.now_local();
                    let due = NaiveTime::parse_from_str(&time, "%H:%M")
                        .map(|value| now.time() >= value)
                        .unwrap_or(false);
                    if enabled && due {
                        let local_date = now.format("%Y-%m-%d").to_string();
                        let existing = self
                            .database
                            .list_daily_summaries(account_id.clone(), 100)
                            .await
                            .unwrap_or_default();
                        for group_id in groups.into_iter().filter(|group_id| {
                            !existing.iter().any(|summary| {
                                summary.group_id == *group_id && summary.local_date == local_date
                            })
                        }) {
                            if !self
                                .database
                                .claim_summary_run(account_id.clone(), group_id, local_date.clone())
                                .await
                                .unwrap_or(false)
                            {
                                continue;
                            }
                            let work_id = runtime_lane_id("summary", &account_id, group_id);
                            self.coordination.tracker.start_named(
                                &work_id,
                                "summary",
                                "生成每日摘要",
                                "计划任务",
                                None,
                            );
                            match self
                                .generate_scheduled_summary(&account_id, group_id, &local_date)
                                .await
                            {
                                Ok(summary) => {
                                    self.coordination.tracker.finish(&work_id, "succeeded", "");
                                    let _ = self
                                        .database
                                        .finish_summary_run(
                                            account_id.clone(),
                                            group_id,
                                            local_date.clone(),
                                            true,
                                            String::new(),
                                        )
                                        .await;
                                    let _ = app.emit("task-progress", serde_json::json!({"kind":"dailySummary","groupId":group_id,"summaryId":summary.id,"success":true}));
                                }
                                Err(error) => {
                                    self.coordination.tracker.finish(
                                        &work_id,
                                        "failed",
                                        &error.message,
                                    );
                                    let _ = self
                                        .database
                                        .finish_summary_run(
                                            account_id.clone(),
                                            group_id,
                                            local_date.clone(),
                                            false,
                                            error.message.clone(),
                                        )
                                        .await;
                                    let _ = self
                                        .database
                                        .record_audit(AuditEvent {
                                            id: 0,
                                            account_id: account_id.clone(),
                                            group_id,
                                            user_id: 0,
                                            actor: "DH BOT".into(),
                                            event: "daily_summary_failed".into(),
                                            level: "error".into(),
                                            details: error.message.clone(),
                                            created_at: Utc::now(),
                                        })
                                        .await;
                                    let _ = app.emit("task-progress", serde_json::json!({"kind":"dailySummary","groupId":group_id,"success":false,"error":error.message}));
                                }
                            }
                        }
                    }
                }
            }
            tokio::select! { _ = self.coordination.wake.notified() => {}, _ = sleep(Duration::from_secs(30)) => {}, _ = self.shutdown.cancelled() => break }
        }
    }

    async fn generate_scheduled_summary(
        &self,
        account_id: &str,
        group_id: i64,
        local_date: &str,
    ) -> AppResult<DailySummary> {
        let messages = self
            .database
            .list_messages(account_id.to_string(), Some(group_id), 1000)
            .await?
            .into_iter()
            .filter(|message| {
                message
                    .sent_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d")
                    .to_string()
                    == local_date
            })
            .take(200)
            .collect::<Vec<_>>();
        if messages.is_empty() {
            return Err(AppError::new(
                "summary_empty",
                "当前群今天还没有可供总结的消息",
            ));
        }
        let (provider, _) = self
            .ai_provider(account_id, ai::AI_PROVIDER_TIMEOUT)
            .await?;
        let request = AiRequest {
            version: "1",
            event_id: uuid::Uuid::new_v4().to_string(),
            persona: ai::PERSONA,
            group_id,
            group_name: format!("群 {group_id}"),
            member_id: 0,
            member_name: "每日摘要".into(),
            member_role: "system".into(),
            message_id: format!("summary-{group_id}-{local_date}"),
            message: "请用简洁中文总结今天的群聊重点，只输出摘要内容。".into(),
            recent_context: messages
                .iter()
                .rev()
                .map(|message| AiContextMessage {
                    user_id: message.user_id,
                    name: message.sender_name.clone(),
                    text: message.text.clone(),
                    time: message.sent_at.timestamp_millis(),
                })
                .collect(),
            knowledge: Vec::new(),
        };
        let decision = provider.decide(&request).await?;
        if decision.reply.trim().is_empty() {
            return Err(AppError::new("summary_empty", "AI 没有返回摘要内容"));
        }
        let summary = DailySummary {
            id: 0,
            account_id: account_id.into(),
            group_id,
            local_date: local_date.into(),
            content: decision.reply,
            source: "scheduled-ai".into(),
            created_at: self.clock.now_utc(),
        };
        let id = self.database.save_daily_summary(summary.clone()).await?;
        Ok(DailySummary { id, ..summary })
    }
}
