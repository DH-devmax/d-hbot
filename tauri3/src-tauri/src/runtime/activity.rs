//! 活动运行准备、文案生成与派发。
//!
//! 本文件是 `BackendRuntime` 的一部分；字段与私有辅助函数通过
//! `use super::*` 从 `runtime/mod.rs` 继承。

use super::*;

impl BackendRuntime {
    #[allow(clippy::too_many_arguments)]
    async fn enqueue_activity_text_effect(
        &self,
        run_id: i64,
        account_id: &str,
        group_id: i64,
        text: &str,
        source: &str,
        dedupe_key: String,
        metadata: Value,
    ) -> AppResult<()> {
        if self.gateway.calibration_active() {
            return Err(AppError::new(
                "calibration_active",
                "真实校准期间已暂停自动副作用，避免后台任务修改测试群状态",
            ));
        }
        let mut payload = serde_json::json!({"text":text,"purpose":"activity"});
        if let (Some(target), Some(extra)) = (payload.as_object_mut(), metadata.as_object()) {
            target.extend(extra.clone());
        }
        let enqueued = self
            .database
            .enqueue_activity_effect(
                run_id,
                text.into(),
                source.into(),
                EffectOutboxRequest {
                    account_id: account_id.into(),
                    group_id,
                    effect_type: "send_text".into(),
                    payload_json: payload.to_string(),
                    dedupe_key,
                },
            )
            .await?;
        if enqueued.inserted && matches!(enqueued.state.as_str(), "queued" | "retry") {
            let work_id = runtime_lane_id("send_text", account_id, group_id);
            self.coordination.tracker.enqueue(
                &work_id,
                "write",
                effect_label("send_text"),
                "群操作",
            );
            self.coordination.notify();
        }
        Ok(())
    }

    pub(super) async fn activity_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((_, account_id)) = self.gateway.session_identity().await {
                    let _ = self
                        .database
                        .generate_due_activity_runs(
                            account_id.clone(),
                            self.clock.now_utc(),
                            crate::activities::MISSED_RUN_GRACE_MINUTES,
                            20,
                        )
                        .await;
                    let runs = self
                        .database
                        .claim_activity_preparations(account_id.clone(), self.clock.now_utc(), 8)
                        .await
                        .unwrap_or_default();
                    let results = futures_util::future::join_all(
                        runs.into_iter().map(|due| self.process_activity_run(due)),
                    )
                    .await;
                    for (activity_id, run_id, success, details) in results {
                        let _ = app.emit(
                            "activity-updated",
                            serde_json::json!({"activityId":activity_id,"runId":run_id,"success":success,"detail":details}),
                        );
                    }
                }
            }
            tokio::select! { _ = self.coordination.wake.notified() => {}, _ = sleep(Duration::from_secs(15)) => {}, _ = self.shutdown.cancelled() => break }
        }
    }

    async fn process_activity_run(
        &self,
        due: crate::models::DueActivityRun,
    ) -> (i64, i64, bool, String) {
        let work_id = runtime_lane_id("activity", &due.run.account_id, due.run.group_id);
        self.coordination.tracker.start_named(
            &work_id,
            "activity",
            "准备活动文案",
            "活动发布",
            Some(3),
        );
        self.coordination.tracker.progress(&work_id, 1, 3);
        let (text, source, generation_error) = self.prepare_activity_text(&due).await;
        self.coordination.tracker.progress(&work_id, 2, 3);
        let result = self
            .enqueue_activity_text_effect(
                due.run.id,
                &due.run.account_id,
                due.run.group_id,
                &text,
                &source,
                due.run.run_key.clone(),
                serde_json::json!({
                    "activityRunId": due.run.id,
                    "activityId": due.activity.id,
                    "activityName": due.activity.name,
                    "contentSource": source,
                }),
            )
            .await;
        let (level, details, success) = match result {
            Ok(()) => {
                self.coordination.tracker.progress(&work_id, 3, 3);
                (
                    if generation_error.is_empty() {
                        "info"
                    } else {
                        "warning"
                    },
                    serde_json::json!({
                        "activityId": due.activity.id,
                        "runId": due.run.id,
                        "source": source,
                        "generationFallback": !generation_error.is_empty(),
                        "generationError": generation_error,
                    })
                    .to_string(),
                    true,
                )
            }
            Err(error) => {
                let _ = self
                    .database
                    .finish_activity_run(due.run.id, "failed".into(), error.message.clone())
                    .await;
                ("error", error.message, false)
            }
        };
        self.coordination.tracker.finish(
            &work_id,
            if success { "succeeded" } else { "failed" },
            if success { "" } else { &details },
        );
        let _ = self
            .database
            .record_audit(AuditEvent {
                id: 0,
                account_id: due.run.account_id,
                group_id: due.run.group_id,
                user_id: 0,
                actor: "DH BOT".into(),
                event: "activity_queued".into(),
                level: level.into(),
                details: details.clone(),
                created_at: Utc::now(),
            })
            .await;
        (due.activity.id, due.run.id, success, details)
    }

    async fn prepare_activity_text(
        &self,
        due: &crate::models::DueActivityRun,
    ) -> (String, String, String) {
        if !due.activity.ai_optimize {
            return (due.activity.content.clone(), "fixed".into(), String::new());
        }
        let context = self
            .database
            .activity_context(due.run.account_id.clone(), due.run.group_id, 5)
            .await
            .unwrap_or_default();
        let recent_texts = context
            .1
            .into_iter()
            .map(|run| run.text)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>();
        let group_name = self
            .database
            .list_groups(Some(due.run.account_id.clone()))
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|group| group.group_id == due.run.group_id)
            .map(|group| group.name)
            .unwrap_or_else(|| "当前群".into());
        let result = async {
            let _permit = self
                .ai_rule_gate
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| AppError::new("activity_ai_gate", "AI 任务队列已关闭"))?;
            let (provider, _) = self
                .ai_provider(&due.run.account_id, ai::AI_PROVIDER_TIMEOUT)
                .await?;
            let mut request = AiRequest {
                version: "1",
                event_id: uuid::Uuid::new_v4().to_string(),
                persona: ai::PERSONA,
                group_id: due.run.group_id,
                group_name: group_name.clone(),
                member_id: 0,
                member_name: "活动调度器".into(),
                member_role: "system".into(),
                message_id: due.run.run_key.clone(),
                message: crate::activities::ai_prompt(&due.activity, &group_name, &recent_texts),
                recent_context: Vec::new(),
                knowledge: Vec::new(),
            };
            // 事实类失败也给一次纠正机会：直接回退到管理员原文虽然安全，但只要提示
            // 模型「别改数字、别加优惠」通常就能过，没必要浪费这一轮生成。
            let base_prompt = request.message.clone();
            let mut retry_hint = "";
            let mut last_error =
                AppError::new("activity_ai_text", "AI 活动文案未通过校验".to_string());
            for attempt in 0..2 {
                if attempt > 0 {
                    request.message = format!("{base_prompt}{retry_hint}");
                }
                let decision = provider.decide(&request).await?;
                match crate::activities::validate_generated_text(
                    &due.activity.content,
                    &decision.reply,
                    &recent_texts,
                ) {
                    Ok(text) => return Ok(text),
                    Err(error) => {
                        retry_hint = match error.code.as_str() {
                            "activity_ai_duplicate" => {
                                "\n本次必须换一种不同的表达，不能复用近期文案。"
                            }
                            "activity_ai_facts" => {
                                "\n本次必须原样保留管理员原文里的每个日期、时间、金额、数字和链接，顺序也不能调换。"
                            }
                            "activity_ai_invented_facts" => {
                                "\n本次不能出现管理员原文里没有的数字、金额或链接，只允许改写措辞。"
                            }
                            _ => return Err(error),
                        };
                        last_error = error;
                    }
                }
            }
            Err(last_error)
        }
        .await;
        match result {
            Ok(text) => (text, "ai".into(), String::new()),
            Err(error) => (
                due.activity.content.clone(),
                "ai-fallback".into(),
                redact(&error.message),
            ),
        }
    }
}
