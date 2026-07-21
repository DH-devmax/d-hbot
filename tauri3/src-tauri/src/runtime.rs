use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveTime, Utc};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Notify};
use tokio::time::sleep;

use crate::ai::{self, AiConfig, AiContextMessage, AiProvider, AiRequest, ConfiguredProvider};
use crate::database::DatabaseExecutor;
use crate::diagnostics::Logger;
use crate::error::{AppError, AppResult};
use crate::gateway::{
    ConnectionStatus, GatewayEvent, GatewayReceipt, GatewayRecord, GatewayRecordKind,
    RuntimeGateway,
};
use crate::models::{
    Account, ActionRecord, AuditEvent, DailySummary, EffectOutboxItem, EffectOutboxRequest,
    GatewayInboxEvent, Group, Member, MemberRef, Message, RuleAction, TaskItem,
};
use crate::secrets::SecretStore;
use crate::{knowledge, moderation, prediction, scheduler};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProgress {
    pub phase: String,
    pub detail: String,
    pub processed: usize,
}

#[derive(Debug, Clone)]
struct IncomingJob {
    sequence: u64,
    message: Message,
}

#[derive(Clone)]
pub struct BackendRuntime {
    database: DatabaseExecutor,
    gateway: Arc<dyn RuntimeGateway>,
    secrets: SecretStore,
    shutdown: Arc<Notify>,
    logger: Logger,
}

impl BackendRuntime {
    pub fn new(
        database: DatabaseExecutor,
        gateway: Arc<dyn RuntimeGateway>,
        secrets: SecretStore,
        shutdown: Arc<Notify>,
        logger: Logger,
    ) -> Self {
        Self {
            database,
            gateway,
            secrets,
            shutdown,
            logger,
        }
    }

    pub fn spawn(self, app: AppHandle) -> Vec<tauri::async_runtime::JoinHandle<()>> {
        let mut workers = Vec::with_capacity(6);
        let card_queue = self.clone();
        let card_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            card_queue.card_queue_loop(card_app).await;
        }));
        let scheduler = self.clone();
        let scheduler_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            scheduler.schedule_loop(scheduler_app).await;
        }));
        let connection = self.clone();
        let connection_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            connection.connection_loop(connection_app).await;
        }));
        let reminders = self.clone();
        let reminders_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            reminders.reminder_loop(reminders_app).await;
        }));
        let summaries = self.clone();
        let summaries_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            summaries.summary_loop(summaries_app).await;
        }));
        let effects = self.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            effects.effect_loop(app).await;
        }));
        workers
    }

    async fn enqueue_effect(
        &self,
        account_id: &str,
        group_id: i64,
        effect_type: &str,
        payload: Value,
        dedupe_key: String,
    ) -> AppResult<()> {
        self.database
            .enqueue_effect(EffectOutboxRequest {
                account_id: account_id.into(),
                group_id,
                effect_type: effect_type.into(),
                payload_json: payload.to_string(),
                dedupe_key,
            })
            .await?;
        Ok(())
    }

    async fn enqueue_text_effect(
        &self,
        account_id: &str,
        group_id: i64,
        text: &str,
        purpose: &str,
        dedupe_key: String,
        metadata: Value,
    ) -> AppResult<()> {
        let mut payload = serde_json::json!({"text":text,"purpose":purpose});
        if let (Some(target), Some(source)) = (payload.as_object_mut(), metadata.as_object()) {
            target.extend(source.clone());
        }
        self.enqueue_effect(account_id, group_id, "send_text", payload, dedupe_key)
            .await
    }

    async fn effect_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok(items) = self.database.claim_effect_outbox(None, 50).await {
                    for item in items {
                        self.dispatch_effect(Some(&app), item).await;
                    }
                }
            }
            tokio::select! {
                _ = sleep(Duration::from_millis(500)) => {},
                _ = self.shutdown.notified() => break,
            }
        }
    }

    async fn dispatch_effect(&self, app: Option<&AppHandle>, item: EffectOutboxItem) {
        let payload: Value = serde_json::from_str(&item.payload_json).unwrap_or(Value::Null);
        let user_id = payload.get("userId").and_then(Value::as_i64).unwrap_or(0);
        let duration = payload
            .get("durationSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let capabilities = self.gateway.capabilities();
        let capability = match item.effect_type.as_str() {
            "recall" => Some(&capabilities.recall),
            "mute" | "unmute" => Some(&capabilities.mute),
            "remove" => Some(&capabilities.remove_member),
            "group_mute" => Some(&capabilities.group_mute),
            _ => None,
        };
        let result = if capability.is_some_and(|value| !value.is_supported()) {
            Err(AppError::new(
                "capability_unverified",
                "当前旺商聊版本尚未完成该协议能力校准",
            ))
        } else {
            match item.effect_type.as_str() {
                "send_text" => {
                    self.gateway
                        .send_text(
                            item.group_id,
                            payload
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )
                        .await
                }
                "recall" => {
                    self.gateway
                        .recall(
                            item.group_id,
                            user_id,
                            payload
                                .get("serverMessageId")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )
                        .await
                }
                "mute" => self.gateway.mute(item.group_id, user_id, duration).await,
                "unmute" => self.gateway.unmute(item.group_id, user_id).await,
                "remove" => self.gateway.remove_member(item.group_id, user_id).await,
                "blacklist" => self
                    .database
                    .set_member_blacklisted(item.account_id.clone(), item.group_id, user_id, true)
                    .await
                    .map(|_| GatewayReceipt::succeeded("database.member.blacklist")),
                "group_mute" => {
                    self.gateway
                        .set_group_mute(
                            item.group_id,
                            payload
                                .get("muted")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        )
                        .await
                }
                _ => Err(AppError::new("effect_unsupported", "当前副作用类型未开放")),
            }
        };
        let (success, error_text, receipt) = match result {
            Ok(receipt) => (true, String::new(), receipt),
            Err(error) => (
                false,
                error.message.clone(),
                GatewayReceipt::failed(&item.effect_type, &error),
            ),
        };
        let receipt_json = serde_json::to_string(&receipt).unwrap_or_default();
        let _ = self
            .database
            .finish_effect_outbox(item.id, success, error_text.clone(), receipt_json.clone())
            .await;

        if let Some(task_id) = payload.get("taskId").and_then(Value::as_i64) {
            if success {
                let _ = self
                    .database
                    .finish_task_reminder(task_id, true, String::new())
                    .await;
            } else if item.attempts >= 5 {
                let _ = self
                    .database
                    .finish_task_reminder(task_id, false, error_text.clone())
                    .await;
            }
            if let Some(app) = app {
                let _ = app.emit(
                    "task-progress",
                    serde_json::json!({"kind":"reminder","taskId":task_id,"success":success,"detail":error_text}),
                );
            }
        }
        if let Some(run_key) = payload.get("scheduleRunKey").and_then(Value::as_str) {
            if success || item.attempts >= 5 {
                let _ = self
                    .database
                    .finish_schedule_run(run_key.to_string(), success, error_text.clone())
                    .await;
            }
            if let Some(app) = app {
                let _ = app.emit(
                    "schedule-updated",
                    serde_json::json!({"runKey":run_key,"success":success,"error":error_text}),
                );
            }
        }
        if success {
            if let Some(card_job_id) = payload.get("cardJobId").and_then(Value::as_i64) {
                let _ = self.database.mark_card_welcome_sent(card_job_id).await;
            }
        }
        if payload
            .get("recordAction")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let kind = payload
                .get("actionKind")
                .and_then(Value::as_str)
                .unwrap_or(&item.effect_type);
            let action = ActionRecord {
                id: 0,
                account_id: item.account_id.clone(),
                group_id: item.group_id,
                user_id,
                message_id: payload.get("messageId").and_then(Value::as_i64),
                rule_id: payload.get("ruleId").and_then(Value::as_i64),
                kind: kind.into(),
                mode: payload
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("automatic")
                    .into(),
                duration_seconds: duration,
                reason: payload
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                success,
                error: error_text.clone(),
                receipt_json: receipt_json.clone(),
                dedupe_key: item.dedupe_key.clone(),
                created_at: Utc::now(),
            };
            let _ = self.database.record_action(action).await;
            if success {
                if let Some(rule_ids) = payload.get("contributorRuleIds").and_then(Value::as_array)
                {
                    for rule_id in rule_ids.iter().filter_map(Value::as_i64) {
                        let _ = self
                            .database
                            .mark_rule_executed(
                                rule_id,
                                item.account_id.clone(),
                                item.group_id,
                                user_id,
                                Utc::now(),
                            )
                            .await;
                    }
                }
            }
            if let Some(app) = app {
                let _ = app.emit("action-recorded", json_action(kind, success, &error_text));
            }
        }
        let _ = self
            .database
            .record_audit(AuditEvent {
                id: 0,
                account_id: item.account_id,
                group_id: item.group_id,
                user_id,
                actor: "DH BOT".into(),
                event: "effect_dispatched".into(),
                level: if success { "info" } else { "error" }.into(),
                details: serde_json::json!({
                    "effect": item.effect_type,
                    "success": success,
                    "error": error_text,
                    "receipt": receipt,
                })
                .to_string(),
                created_at: Utc::now(),
            })
            .await;
    }

    fn worker_for(
        &self,
        workers: &mut HashMap<i64, mpsc::Sender<IncomingJob>>,
        group_id: i64,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
    ) -> mpsc::Sender<IncomingJob> {
        workers
            .entry(group_id)
            .or_insert_with(|| {
                let (sender, mut receiver) = mpsc::channel::<IncomingJob>(128);
                let runtime = self.clone();
                let app_handle = app.clone();
                let account = account_id.to_string();
                tokio::spawn(async move {
                    while let Some(job) = receiver.recv().await {
                        if runtime
                            .database
                            .claim_message(job.message.id)
                            .await
                            .unwrap_or(false)
                        {
                            let result = runtime
                                .process_job(&app_handle, &account, sender_id, job.clone())
                                .await;
                            let (success, error) = match result {
                                Ok(()) => (true, String::new()),
                                Err(error) => (false, error.message),
                            };
                            let _ = runtime
                                .database
                                .finish_message_processing(job.message.id, success, error)
                                .await;
                        }
                    }
                });
                sender
            })
            .clone()
    }

    async fn summary_loop(&self, app: AppHandle) {
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
                    let now = Local::now();
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
                            match self
                                .generate_scheduled_summary(&account_id, group_id, &local_date)
                                .await
                            {
                                Ok(summary) => {
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
            tokio::select! { _ = sleep(Duration::from_secs(30)) => {}, _ = self.shutdown.notified() => break }
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
        let provider = ConfiguredProvider::new(AiConfig {
            base_url: self
                .database
                .get_setting("ai.base_url".into())
                .await?
                .unwrap_or_default(),
            webhook_url: self
                .database
                .get_setting("ai.webhook_url".into())
                .await?
                .unwrap_or_default(),
            model: self
                .database
                .get_setting("ai.model".into())
                .await?
                .unwrap_or_else(|| "deepseek-v4-pro".into()),
            api_key: self.gateway_secret("ai.api_key").await,
            timeout: Duration::from_secs(30),
        })?;
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
            created_at: Utc::now(),
        };
        let id = self.database.save_daily_summary(summary.clone()).await?;
        Ok(DailySummary { id, ..summary })
    }

    async fn reminder_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok(tasks) = self.database.claim_due_task_reminders(Utc::now(), 20).await {
                    for task in tasks {
                        let text = format!(
                            "任务提醒：{}{}",
                            task.title,
                            if task.description.trim().is_empty() {
                                String::new()
                            } else {
                                format!("\n{}", task.description)
                            }
                        );
                        let result = self
                            .enqueue_text_effect(
                                &task.account_id,
                                task.group_id,
                                &text,
                                "task-reminder",
                                format!("task-reminder:{}", task.id),
                                serde_json::json!({"taskId":task.id,"userId":task.assignee_id}),
                            )
                            .await;
                        let (level, details, success) = match result {
                            Ok(()) => (
                                "info",
                                format!("任务“{}”提醒已进入发送队列", task.title),
                                true,
                            ),
                            Err(error) => {
                                let _ = self
                                    .database
                                    .finish_task_reminder(task.id, false, error.message.clone())
                                    .await;
                                (
                                    "error",
                                    format!("任务“{}”提醒入队失败：{}", task.title, error.message),
                                    false,
                                )
                            }
                        };
                        let _ = self
                            .database
                            .record_audit(AuditEvent {
                                id: 0,
                                account_id: task.account_id.clone(),
                                group_id: task.group_id,
                                user_id: task.assignee_id,
                                actor: "DH BOT".into(),
                                event: "task_reminder_queued".into(),
                                level: level.into(),
                                details: details.clone(),
                                created_at: Utc::now(),
                            })
                            .await;
                        let _ = app.emit("task-progress", serde_json::json!({"kind":"reminder","taskId":task.id,"success":success,"detail":details}));
                    }
                }
            }
            tokio::select! { _ = sleep(Duration::from_secs(30)) => {}, _ = self.shutdown.notified() => break }
        }
    }

    async fn card_queue_loop(&self, app: AppHandle) {
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
                    if let Ok(Some(job)) =
                        self.database.claim_next_card_job(account_id.clone()).await
                    {
                        let paused = self
                            .database
                            .get_setting(format!("card.paused.{}.{}", job.account_id, job.group_id))
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
                                .map(|_| ())
                        };
                        let result = match result {
                            Ok(()) => {
                                sleep(Duration::from_millis(500)).await;
                                match self.gateway.list_members(job.group_id).await {
                                    Ok(roster) => {
                                        if roster.members.iter().any(|member| {
                                            (member.user_id == job.user_id
                                                || (!job.nim_id.is_empty()
                                                    && member.nim_id == job.nim_id))
                                                && member.card_name == job.desired_name
                                        }) {
                                            Ok(())
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
                        let (success, error) = match result {
                            Ok(()) => (true, String::new()),
                            Err(error) => (false, error.message),
                        };
                        let _ = self
                            .database
                            .finish_card_job(job.clone(), success, error.clone())
                            .await;
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
                                level: if success { "info" } else { "error" }.into(),
                                details: if success {
                                    format!("群名片已改为“{}”", job.desired_name)
                                } else {
                                    error.clone()
                                },
                                created_at: Utc::now(),
                            })
                            .await;
                        let _ = app.emit(
                            "task-progress",
                            serde_json::json!({
                                "kind":"cardRename",
                                "groupId":job.group_id,
                                "userId":job.user_id,
                                "state":if success { "succeeded" } else if job.attempts >= 5 { "failed" } else { "retry" },
                                "error":error
                            }),
                        );
                        tokio::select! {
                            _ = sleep(Duration::from_millis(500)) => {},
                            _ = self.shutdown.notified() => break,
                        }
                        continue;
                    }
                }
            }
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {},
                _ = self.shutdown.notified() => break,
            }
        }
    }

    async fn is_group_manager(&self, sender_id: i64, group_id: i64) -> bool {
        self.gateway
            .list_members(group_id)
            .await
            .ok()
            .and_then(|roster| {
                roster
                    .members
                    .into_iter()
                    .find(|member| member.user_id == sender_id)
            })
            .map(|member| matches!(member.role.as_str(), "owner" | "admin"))
            .unwrap_or(false)
    }

    async fn sync_members(&self, account_id: &str, sender_id: i64) {
        let groups = match self.gateway.list_groups().await {
            Ok(groups) => groups,
            Err(_) => return,
        };
        for group in groups {
            let roster = match self.gateway.list_members(group.group_id).await {
                Ok(roster) => roster,
                Err(_) => continue,
            };
            let existing = self
                .database
                .list_members(account_id.to_string(), group.group_id)
                .await
                .unwrap_or_default();
            let baseline = existing.is_empty();
            let mut present_ids = HashSet::new();
            let mut newly_discovered = HashSet::new();
            for mut member in roster.members {
                present_ids.insert(member.user_id);
                member.account_id = account_id.to_string();
                if let Some(saved) = existing
                    .iter()
                    .find(|saved| saved.user_id == member.user_id)
                {
                    member.original_card_name = saved.original_card_name.clone();
                    member.managed_card_name = saved.managed_card_name.clone();
                    member.card_suffix = saved.card_suffix.clone();
                    member.locked_card_name = saved.locked_card_name.clone();
                    member.violation_count = saved.violation_count;
                    member.blacklisted = saved.blacklisted;
                    member.join_source = saved.join_source.clone();
                    member.prompt_read = saved.prompt_read;
                    member.discovered_at = saved.discovered_at;
                    member.joined_at = saved.joined_at;
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
                    newly_discovered.insert(member.user_id);
                }
                member.present = true;
                member.last_seen_at = Utc::now();
                member.updated_at = member.last_seen_at;
                let _ = self.database.upsert_member(member.clone()).await;
                if !baseline && newly_discovered.contains(&member.user_id) && member.blacklisted {
                    let removed = self
                        .gateway
                        .remove_member(group.group_id, member.user_id)
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
                            level: if removed { "info" } else { "error" }.into(),
                            details: if removed {
                                "黑名单成员重新入群，已自动移出"
                            } else {
                                "黑名单成员重新入群，自动移出失败"
                            }
                            .into(),
                            created_at: Utc::now(),
                        })
                        .await;
                }
            }
            let automatic = self
                .database
                .get_setting(format!("card.auto.{account_id}.{}", group.group_id))
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some("true");
            if !baseline && automatic && !newly_discovered.is_empty() {
                let prefix = self
                    .database
                    .get_setting(format!("card.prefix.{account_id}.{}", group.group_id))
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "DH".into());
                if let Ok(preview) = crate::cardnames::preview(
                    group.group_id,
                    &prefix,
                    self.database
                        .list_members(account_id.to_string(), group.group_id)
                        .await
                        .unwrap_or_default(),
                    sender_id,
                ) {
                    for plan in preview.items.into_iter().filter(|plan| {
                        plan.status == "planned" && newly_discovered.contains(&plan.member.user_id)
                    }) {
                        let _ = self
                            .database
                            .enqueue_card_job(account_id.to_string(), group.group_id, plan, false)
                            .await;
                    }
                }
            }
            if roster.complete && !baseline {
                let missing = self
                    .database
                    .mark_members_not_present(
                        account_id.to_string(),
                        group.group_id,
                        present_ids.iter().copied().collect::<Vec<_>>(),
                    )
                    .await
                    .unwrap_or_default();
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
                            details: "成员同步确认已离群".into(),
                            created_at: Utc::now(),
                        })
                        .await;
                }
            }
        }
    }

    async fn schedule_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((sender_id, account_id)) = self.gateway.session_identity().await {
                    if let Ok(schedules) = self.database.list_schedules(account_id.clone()).await {
                        for schedule in schedules.into_iter().filter(|schedule| schedule.enabled) {
                            for group_id in &schedule.group_ids {
                                let Ok(decision) =
                                    scheduler::evaluate(&schedule, *group_id, chrono::Local::now())
                                else {
                                    continue;
                                };
                                if !self
                                    .database
                                    .claim_schedule_run(
                                        schedule.id,
                                        account_id.clone(),
                                        *group_id,
                                        decision.last_action.clone(),
                                        decision.run_key.clone(),
                                    )
                                    .await
                                    .unwrap_or(false)
                                {
                                    continue;
                                }
                                let allowed = self
                                    .gateway
                                    .list_members(*group_id)
                                    .await
                                    .ok()
                                    .and_then(|roster| {
                                        roster
                                            .members
                                            .into_iter()
                                            .find(|member| member.user_id == sender_id)
                                    })
                                    .map(|member| matches!(member.role.as_str(), "owner" | "admin"))
                                    .unwrap_or(false);
                                let result = if allowed
                                    && self.gateway.capabilities().group_mute.is_supported()
                                {
                                    self.enqueue_effect(
                                        &account_id,
                                        *group_id,
                                        "group_mute",
                                        serde_json::json!({
                                            "muted":decision.should_mute,
                                            "scheduleRunKey":decision.run_key,
                                            "userId":sender_id,
                                        }),
                                        format!("schedule:{}", decision.run_key),
                                    )
                                    .await
                                } else if !allowed {
                                    Err(crate::error::AppError::new(
                                        "management_required",
                                        "需要将账号权限设置为管理",
                                    ))
                                } else {
                                    Err(crate::error::AppError::new(
                                        "capability_unverified",
                                        "当前旺商聊版本尚未完成全群禁言能力校准",
                                    ))
                                };
                                let (success, error) = match result {
                                    Ok(()) => (true, String::new()),
                                    Err(error) => (false, error.message),
                                };
                                if !success {
                                    let _ = self
                                        .database
                                        .finish_schedule_run(
                                            decision.run_key.clone(),
                                            false,
                                            error.clone(),
                                        )
                                        .await;
                                }
                                let _ = self
                                    .database
                                    .record_audit(AuditEvent {
                                        id: 0,
                                        account_id: account_id.clone(),
                                        group_id: *group_id,
                                        user_id: sender_id,
                                        actor: "DH BOT".into(),
                                        event: if decision.should_mute {
                                            "schedule_close_group"
                                        } else {
                                            "schedule_open_group"
                                        }
                                        .into(),
                                        level: if success { "info" } else { "error" }.into(),
                                        details: if success {
                                            format!("计划“{}”已进入执行队列", schedule.name)
                                        } else {
                                            error.clone()
                                        },
                                        created_at: Utc::now(),
                                    })
                                    .await;
                                let _ = app.emit("schedule-updated", serde_json::json!({"scheduleId":schedule.id,"groupId":group_id,"success":success,"error":error,"nextAt":decision.next_at}));
                            }
                        }
                    }
                }
            }
            tokio::select! { _ = sleep(Duration::from_secs(30)) => {}, _ = self.shutdown.notified() => break }
        }
    }

    async fn connection_loop(&self, app: AppHandle) {
        let mut workers: HashMap<i64, mpsc::Sender<IncomingJob>> = HashMap::new();
        let mut last_roster_sync = Utc::now() - chrono::Duration::seconds(60);
        let mut last_retry_scan = Utc::now() - chrono::Duration::seconds(5);
        loop {
            let diagnostic = self.gateway.diagnose().await;
            let _ = app.emit("connection-status", &diagnostic);
            let _ = app.emit("gateway-capabilities", self.gateway.capabilities());
            if diagnostic.status != ConnectionStatus::Ready {
                tokio::select! { _ = sleep(Duration::from_secs(5)) => {}, _ = self.shutdown.notified() => break }
                continue;
            }
            let (sender_id, account_id) = match self.gateway.session_identity().await {
                Ok(identity) => identity,
                Err(error) => {
                    self.logger.write("WARN", &error.message);
                    let _ = app.emit("connection-error", &error);
                    tokio::select! { _ = sleep(Duration::from_secs(5)) => {}, _ = self.shutdown.notified() => break };
                    continue;
                }
            };
            let now = Utc::now();
            let _ = self
                .database
                .upsert_account(Account {
                    id: account_id.clone(),
                    display_name: diagnostic.nim_account.clone(),
                    role: "unknown".into(),
                    discovered_at: now,
                    updated_at: now,
                })
                .await;
            if let Ok(groups) = self.gateway.list_groups().await {
                for group in groups {
                    let _ = self.database.upsert_group(group).await;
                }
            }
            if self.gateway.install_message_listener().await.is_err() {
                tokio::select! { _ = sleep(Duration::from_secs(5)) => {}, _ = self.shutdown.notified() => break }
                continue;
            }
            let _ = app.emit(
                "sync-progress",
                RuntimeProgress {
                    phase: "listening".into(),
                    detail: "消息监听已就绪".into(),
                    processed: 0,
                },
            );
            // Recover messages that were durably stored before a restart or a
            // temporary protocol failure. They are already acknowledged at the
            // source, so only the local worker needs to be resumed.
            if let Ok(pending) = self
                .database
                .list_pending_messages(account_id.clone(), 1000)
                .await
            {
                for message in pending {
                    let sender = self.worker_for(
                        &mut workers,
                        message.group_id,
                        &app,
                        &account_id,
                        sender_id,
                    );
                    let _ = sender
                        .send(IncomingJob {
                            sequence: message.sequence.max(0) as u64,
                            message,
                        })
                        .await;
                }
            }
            loop {
                if Utc::now()
                    .signed_duration_since(last_roster_sync)
                    .num_seconds()
                    >= 60
                {
                    self.sync_members(&account_id, sender_id).await;
                    last_roster_sync = Utc::now();
                }
                if Utc::now()
                    .signed_duration_since(last_retry_scan)
                    .num_seconds()
                    >= 2
                {
                    if let Ok(pending) = self
                        .database
                        .list_pending_messages(account_id.clone(), 200)
                        .await
                    {
                        for message in pending {
                            let sender = self.worker_for(
                                &mut workers,
                                message.group_id,
                                &app,
                                &account_id,
                                sender_id,
                            );
                            let _ = sender.try_send(IncomingJob {
                                sequence: message.sequence.max(0) as u64,
                                message,
                            });
                        }
                    }
                    if let Ok(inbox) = self
                        .database
                        .claim_gateway_inbox(Some(account_id.clone()), 100)
                        .await
                    {
                        for item in inbox {
                            if item.event_type != "message" {
                                let _ = self
                                    .database
                                    .finish_gateway_inbox(
                                        item.id,
                                        false,
                                        "成员事件等待名单同步确认".into(),
                                    )
                                    .await;
                                continue;
                            }
                            let mut raw: Value = match serde_json::from_str(&item.payload_json) {
                                Ok(value) => value,
                                Err(error) => {
                                    let _ = self
                                        .database
                                        .finish_gateway_inbox(
                                            item.id,
                                            false,
                                            format!("事件载荷解析失败：{error}"),
                                        )
                                        .await;
                                    continue;
                                }
                            };
                            if let Some(object) = raw.as_object_mut() {
                                object.insert(
                                    "seq".into(),
                                    Value::from(item.bridge_sequence.max(0) as u64),
                                );
                                object.insert("source".into(), Value::from("gateway-inbox"));
                            }
                            match self.normalize_message(&account_id, sender_id, &raw).await {
                                Ok(mut job) => {
                                    match self.database.insert_message(job.message.clone()).await {
                                        Ok(persisted) => {
                                            job.message.id = persisted.id;
                                            let _ = self
                                                .database
                                                .mark_message_acknowledged(persisted.id)
                                                .await;
                                            let _ = self
                                                .database
                                                .finish_gateway_inbox(item.id, true, String::new())
                                                .await;
                                            if !persisted.processed {
                                                let sender = self.worker_for(
                                                    &mut workers,
                                                    job.message.group_id,
                                                    &app,
                                                    &account_id,
                                                    sender_id,
                                                );
                                                let _ = sender.try_send(job);
                                            }
                                        }
                                        Err(error) => {
                                            let _ = self
                                                .database
                                                .finish_gateway_inbox(item.id, false, error.message)
                                                .await;
                                        }
                                    }
                                }
                                Err(reason) if reason == "解码失败且群身份未映射" => {
                                    let _ = self
                                        .database
                                        .finish_gateway_inbox(item.id, false, reason)
                                        .await;
                                }
                                Err(_) => {
                                    let _ = self
                                        .database
                                        .finish_gateway_inbox(item.id, true, String::new())
                                        .await;
                                }
                            }
                        }
                    }
                    last_retry_scan = Utc::now();
                }
                let batch = match self.gateway.read_batch().await {
                    Ok(value) => value,
                    Err(error) => {
                        self.logger.write("WARN", &error.message);
                        let _ = app.emit("connection-error", &error);
                        break;
                    }
                };
                if batch.dropped > 0 {
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.clone(),
                            group_id: 0,
                            user_id: 0,
                            actor: "DH BOT".into(),
                            event: "gateway_queue_overflow".into(),
                            level: "error".into(),
                            details: format!("桥接队列已丢弃 {} 条记录", batch.dropped),
                            created_at: Utc::now(),
                        })
                        .await;
                }
                let inbox_events = batch
                    .records
                    .iter()
                    .map(|record| GatewayInboxEvent {
                        account_id: account_id.clone(),
                        bridge_session: record.session.clone(),
                        bridge_sequence: record.sequence as i64,
                        event_id: gateway_record_id(record),
                        event_type: match record.kind {
                            GatewayRecordKind::Message => "message",
                            GatewayRecordKind::TeamMemberJoined => "member-joined",
                            GatewayRecordKind::TeamMemberLeft => "member-left",
                            GatewayRecordKind::TeamMemberUpdated => "member-updated",
                            GatewayRecordKind::ConnectionChanged => "connection-changed",
                        }
                        .into(),
                        payload_json: record.payload.to_string(),
                        received_at: Utc::now(),
                    })
                    .collect::<Vec<_>>();
                let mut normalized = Vec::new();
                let mut ignored = Vec::new();
                let mut completed_inbox_ids = Vec::new();
                for record in &batch.records {
                    let inbox_event_id = gateway_record_id(record);
                    if record.kind != GatewayRecordKind::Message {
                        completed_inbox_ids.push(inbox_event_id);
                        continue;
                    }
                    let mut raw = record.payload.clone();
                    if let Some(object) = raw.as_object_mut() {
                        object.insert("seq".into(), Value::from(record.sequence));
                        object.insert("source".into(), Value::from(record.source.clone()));
                    }
                    match self.normalize_message(&account_id, sender_id, &raw).await {
                        Ok(job) => {
                            completed_inbox_ids.push(inbox_event_id);
                            normalized.push((job, raw));
                        }
                        Err(reason) => {
                            if reason != "解码失败且群身份未映射" {
                                completed_inbox_ids.push(inbox_event_id);
                            }
                            ignored.push((record.sequence, reason));
                        }
                    }
                }
                let messages = normalized
                    .iter()
                    .map(|(job, _)| job.message.clone())
                    .collect::<Vec<_>>();
                let (_, persisted_messages) = match self
                    .database
                    .ingest_gateway_batch(inbox_events, messages)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => {
                        self.logger.write("WARN", &error.message);
                        let _ = app.emit("connection-error", &error);
                        tokio::select! { _ = sleep(Duration::from_millis(700)) => {}, _ = self.shutdown.notified() => return }
                        continue;
                    }
                };
                // Events are only applied after the raw ordered batch is durable.
                // CdpGateway reads the same unacknowledged batch when normalizing
                // member callbacks, so this does not advance the source queue.
                for event in self.gateway.poll_events().await.unwrap_or_default() {
                    self.handle_gateway_event(&app, &account_id, event).await;
                }
                for (sequence, reason) in ignored {
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.clone(),
                            group_id: 0,
                            user_id: 0,
                            actor: "DH BOT".into(),
                            event: "message_ignored".into(),
                            level: if reason == "解码失败且群身份未映射" {
                                "warning"
                            } else {
                                "info"
                            }
                            .into(),
                            details: format!("序号={sequence}，{reason}"),
                            created_at: Utc::now(),
                        })
                        .await;
                }
                let ack_sequence = batch
                    .records
                    .last()
                    .map(|record| record.sequence)
                    .unwrap_or(0);
                if ack_sequence > 0 {
                    if let Err(error) = self.gateway.ack(&batch.session, ack_sequence).await {
                        self.logger.write("WARN", &error.message);
                    } else {
                        for persisted in &persisted_messages {
                            let _ = self.database.mark_message_acknowledged(persisted.id).await;
                        }
                        let _ = self
                            .database
                            .mark_gateway_inbox_processed(account_id.clone(), completed_inbox_ids)
                            .await;
                        for ((mut job, raw), persisted) in
                            normalized.into_iter().zip(persisted_messages)
                        {
                            if persisted.processed {
                                continue;
                            }
                            job.message.id = persisted.id;
                            let sender = self.worker_for(
                                &mut workers,
                                job.message.group_id,
                                &app,
                                &account_id,
                                sender_id,
                            );
                            let _ = sender.send(job).await;
                            let _ = app.emit("message-received", &raw);
                        }
                    }
                }
                tokio::select! { _ = sleep(Duration::from_millis(700)) => {}, _ = self.shutdown.notified() => return }
            }
            tokio::select! { _ = sleep(Duration::from_secs(5)) => {}, _ = self.shutdown.notified() => break }
        }
    }

    async fn handle_gateway_event(&self, app: &AppHandle, account_id: &str, event: GatewayEvent) {
        match event {
            GatewayEvent::MemberJoined {
                group_id,
                mut member,
            } => {
                member.account_id = account_id.into();
                if let Some(saved) = self
                    .database
                    .list_members(account_id.to_string(), group_id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|saved| {
                        saved.user_id == member.user_id
                            || (!member.nim_id.is_empty() && saved.nim_id == member.nim_id)
                    })
                {
                    merge_managed_member_state(&mut member, &saved);
                }
                member.join_source = "online-joined".into();
                member.present = true;
                member.prompt_read = false;
                member.joined_at = Some(Utc::now());
                member.discovered_at = Utc::now();
                member.last_seen_at = Utc::now();
                let _ = self.database.upsert_member(member.clone()).await;
                let _ = app.emit("sync-progress", serde_json::json!({"phase":"member-joined","groupId":group_id,"userId":member.user_id,"source":"online-joined"}));
                if member.blacklisted {
                    let result = self.gateway.remove_member(group_id, member.user_id).await;
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "blacklisted_member_rejoined".into(),
                            level: if result.is_ok() { "warning" } else { "error" }.into(),
                            details: result
                                .err()
                                .map(|error| error.message)
                                .unwrap_or_else(|| "黑名单成员重新入群，已移出".into()),
                            created_at: Utc::now(),
                        })
                        .await;
                    return;
                }
                self.enqueue_member_card_job(account_id, group_id, &member)
                    .await;
            }
            GatewayEvent::MemberLeft { group_id, member } => {
                let _ = self
                    .database
                    .mark_member_not_present(account_id.to_string(), group_id, member.user_id)
                    .await;
                let _ = self
                    .database
                    .record_audit(AuditEvent {
                        id: 0,
                        account_id: account_id.into(),
                        group_id,
                        user_id: member.user_id,
                        actor: "DH BOT".into(),
                        event: "member_left".into(),
                        level: "info".into(),
                        details: "收到 NIM 离群事件".into(),
                        created_at: Utc::now(),
                    })
                    .await;
            }
            GatewayEvent::MemberUpdated {
                group_id,
                mut member,
            } => {
                member.account_id = account_id.into();
                let saved = self
                    .database
                    .list_members(account_id.to_string(), group_id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|saved| {
                        saved.user_id == member.user_id
                            || (!member.nim_id.is_empty() && saved.nim_id == member.nim_id)
                    });
                if let Some(saved) = &saved {
                    merge_managed_member_state(&mut member, saved);
                }
                member.last_seen_at = Utc::now();
                member.updated_at = Utc::now();
                let restore_name = saved
                    .as_ref()
                    .map(|saved| saved.locked_card_name.trim())
                    .filter(|locked| !locked.is_empty() && *locked != member.card_name.trim())
                    .map(str::to_string);
                if restore_name.is_some() {
                    member.violation_count += 1;
                }
                let _ = self.database.upsert_member(member.clone()).await;
                if let Some(locked) = restore_name {
                    let result = self
                        .gateway
                        .rename(
                            group_id,
                            &MemberRef {
                                user_id: (member.user_id > 0).then_some(member.user_id),
                                nim_id: (!member.nim_id.is_empty())
                                    .then_some(member.nim_id.clone()),
                            },
                            &locked,
                        )
                        .await;
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "locked_card_restored".into(),
                            level: if result.is_ok() { "warning" } else { "error" }.into(),
                            details: result
                                .err()
                                .map(|error| error.message)
                                .unwrap_or_else(|| format!("群名片已恢复为「{locked}」")),
                            created_at: Utc::now(),
                        })
                        .await;
                }
                let _ = app.emit("sync-progress", serde_json::json!({"phase":"member-updated","groupId":group_id,"userId":member.user_id}));
            }
            GatewayEvent::Message { message } => {
                let _ = app.emit("message-received", &message);
            }
            GatewayEvent::ConnectionChanged { diagnostic } => {
                let _ = app.emit("connection-status", diagnostic);
            }
        }
    }

    async fn enqueue_member_card_job(&self, account_id: &str, group_id: i64, member: &Member) {
        if self
            .database
            .get_setting(format!("card.auto.{account_id}.{group_id}"))
            .await
            .ok()
            .flatten()
            .as_deref()
            != Some("true")
        {
            return;
        }
        let prefix = self
            .database
            .get_setting(format!("card.prefix.{account_id}.{group_id}"))
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "DH".into());
        let members = self
            .database
            .list_members(account_id.to_string(), group_id)
            .await
            .unwrap_or_default();
        let sender_id = self
            .gateway
            .session_identity()
            .await
            .map(|value| value.0)
            .unwrap_or_default();
        if let Ok(preview) = crate::cardnames::preview(group_id, &prefix, members, sender_id) {
            if let Some(plan) = preview
                .items
                .into_iter()
                .find(|plan| plan.member.user_id == member.user_id && plan.status == "planned")
            {
                let _ = self
                    .database
                    .enqueue_card_job(account_id.to_string(), group_id, plan, true)
                    .await;
            }
        }
    }

    async fn normalize_message(
        &self,
        account_id: &str,
        sender_id: i64,
        value: &Value,
    ) -> Result<IncomingJob, String> {
        let sequence = value
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| "缺少桥接序号".to_string())?;
        if value.get("flow").and_then(Value::as_str) == Some("out") {
            return Err("已忽略本账号发出的回显消息".into());
        }
        let decoded = value.get("decoded").unwrap_or(&Value::Null);
        if !decoded.is_null() && decoded.get("msgSession").and_then(value_i64) != Some(2) {
            return Err("已忽略私聊或非群会话消息".into());
        }
        if decoded.is_null()
            && value
                .get("scene")
                .and_then(Value::as_str)
                .is_some_and(|scene| scene != "team")
        {
            return Err("已忽略非群聊 NIM 消息".into());
        }
        let group_id = if let Some(group_id) = decoded.pointer("/to/id").and_then(value_i64) {
            Some(group_id)
        } else if let Some(raw) = value.get("to").and_then(value_i64) {
            self.database
                .list_groups(Some(account_id.to_string()))
                .await
                .ok()
                .is_some_and(|groups| groups.into_iter().any(|group| group.group_id == raw))
                .then_some(raw)
        } else {
            None
        }
        .ok_or_else(|| "解码失败且群身份未映射".to_string())?;
        if group_id <= 0 {
            return Err("群身份无效".into());
        }
        let user_id = decoded
            .pointer("/from/id")
            .and_then(value_i64)
            .or_else(|| value.get("from").and_then(value_i64))
            .unwrap_or(0);
        if user_id <= 0 {
            return Err("发送者身份未识别".into());
        }
        if user_id == sender_id {
            return Err("已忽略本账号消息".into());
        }
        let text = decoded
            .pointer("/content/data")
            .and_then(Value::as_str)
            .or_else(|| value.get("text").and_then(Value::as_str))
            .unwrap_or_else(|| {
                if decoded.is_null() {
                    "[消息解码失败]"
                } else {
                    ""
                }
            })
            .to_string();
        let format = decoded
            .get("msgFormat")
            .and_then(value_i64)
            .or_else(|| value.get("msgFormat").and_then(value_i64))
            .unwrap_or(99);
        let kind = match format {
            0 => "text",
            1 => "image",
            13 => "card",
            7 | 8 => "notice",
            _ => "other",
        };
        let now = Utc::now();
        Ok(IncomingJob {
            sequence,
            message: Message {
                id: 0,
                account_id: account_id.into(),
                group_id,
                server_message_id: value
                    .get("idServer")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .or_else(|| value.get("idClient").and_then(Value::as_str))
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("seq-{sequence}")),
                sequence: sequence as i64,
                user_id,
                sender_name: decoded
                    .pointer("/from/name")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("fromNick").and_then(Value::as_str))
                    .unwrap_or_default()
                    .into(),
                kind: kind.into(),
                text,
                sent_at: timestamp(value.get("time").and_then(value_i64)).unwrap_or(now),
                received_at: now,
                processed_at: None,
                acknowledged_at: None,
                processing_state: "pending".into(),
                attempts: 0,
                next_attempt_at: None,
                last_error: String::new(),
                mentions_json: decoded
                    .get("mentions")
                    .or_else(|| value.get("mentions"))
                    .map(Value::to_string)
                    .unwrap_or_else(|| "[]".into()),
                source_kind: value
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                flow: value
                    .get("flow")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        })
    }

    async fn process_job(
        &self,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
        job: IncomingJob,
    ) -> AppResult<()> {
        if self
            .database
            .get_setting("automation.mode".into())
            .await
            .ok()
            .flatten()
            .as_deref()
            == Some("paused")
        {
            let _ = self
                .database
                .record_audit(AuditEvent {
                    id: 0,
                    account_id: account_id.into(),
                    group_id: job.message.group_id,
                    user_id: job.message.user_id,
                    actor: "DH BOT".into(),
                    event: "automation_paused".into(),
                    level: "info".into(),
                    details: "托盘暂停了自动化执行".into(),
                    created_at: Utc::now(),
                })
                .await;
            return Ok(());
        }
        let Some(group) = self
            .database
            .list_groups(Some(account_id.to_string()))
            .await
            .ok()
            .and_then(|groups| {
                groups
                    .into_iter()
                    .find(|group| group.group_id == job.message.group_id)
            })
        else {
            return Ok(());
        };
        let member = self
            .database
            .list_members(account_id.to_string(), group.group_id)
            .await
            .ok()
            .and_then(|members| {
                members
                    .into_iter()
                    .find(|member| member.user_id == job.message.user_id)
            })
            .unwrap_or_else(|| placeholder_member(account_id, &job.message));
        if member.join_source == "message-discovered" {
            let _ = self.database.upsert_member(member.clone()).await;
        }
        let recent_messages = self
            .database
            .recent_messages(account_id.to_string(), group.group_id, 20)
            .await
            .unwrap_or_default();
        let recent_events = recent_messages
            .iter()
            .map(|message| moderation::RecentEvent {
                user_id: message.user_id,
                kind: message.kind.clone(),
                at: message.sent_at,
            })
            .collect::<Vec<_>>();
        if group.moderation_enabled {
            if let Ok(rules) = self
                .database
                .list_rules(account_id.to_string(), Some(group.group_id))
                .await
            {
                let input = moderation::ModerationInput {
                    member: &member,
                    kind: &job.message.kind,
                    text: &job.message.text,
                    now: Utc::now(),
                    recent: &recent_events,
                    rename_violations: member.violation_count,
                };
                let mut categories = rules
                    .iter()
                    .filter(|rule| rule.enabled && rule.matcher == "semantic")
                    .map(|rule| rule.pattern.trim().to_string())
                    .filter(|category| !category.is_empty())
                    .collect::<Vec<_>>();
                categories.sort();
                categories.dedup();
                let mut decision = if categories.is_empty() {
                    moderation::evaluate_with_classifier(
                        &rules,
                        &input,
                        &moderation::DeterministicSemanticClassifier::default(),
                    )
                } else {
                    match self
                        .classify_semantics(&group, &member, &job.message, &categories)
                        .await
                    {
                        Ok(scores) => {
                            moderation::evaluate_with_semantic_scores(&rules, &input, &scores)
                        }
                        Err(error) => {
                            let _ = self
                                .database
                                .record_audit(AuditEvent {
                                    id: 0,
                                    account_id: account_id.into(),
                                    group_id: group.group_id,
                                    user_id: member.user_id,
                                    actor: "DH BOT".into(),
                                    event: "semantic_classifier_fallback".into(),
                                    level: "warning".into(),
                                    details: error.message,
                                    created_at: Utc::now(),
                                })
                                .await;
                            moderation::evaluate_with_classifier(
                                &rules,
                                &input,
                                &moderation::DeterministicSemanticClassifier::default(),
                            )
                        }
                    }
                };
                let mut executable = Vec::new();
                let mut contributor_rule_ids = Vec::new();
                for matched in &decision.matches {
                    if matches!(matched.mode.as_str(), "auto" | "automatic") {
                        let cooldown = rules
                            .iter()
                            .find(|rule| rule.id == matched.rule_id)
                            .map(|rule| rule.cooldown_seconds)
                            .unwrap_or(0);
                        let allowed = cooldown <= 0
                            || self
                                .database
                                .rule_cooldown_allows(
                                    matched.rule_id,
                                    account_id.to_string(),
                                    group.group_id,
                                    member.user_id,
                                    Utc::now(),
                                    cooldown,
                                )
                                .await
                                .unwrap_or(false);
                        if allowed {
                            contributor_rule_ids.push(matched.rule_id);
                            executable.extend(matched.actions.clone());
                        }
                    }
                }
                decision.actions = moderation::merge_actions(executable);
                self.execute_rule_actions(
                    app,
                    account_id,
                    sender_id,
                    &job.message,
                    &decision.actions,
                    &contributor_rule_ids,
                    decision.automatic,
                )
                .await;
            }
        }
        let ai_has_permission = ["reply", "tasks", "recall", "mute", "remove"]
            .into_iter()
            .map(|name| self.ai_permission(account_id, group.group_id, name, name == "tasks"));
        let ai_has_permission = futures_util::future::join_all(ai_has_permission)
            .await
            .into_iter()
            .any(|allowed| allowed);
        if group.enabled
            && group.ai_enabled
            && ai_has_permission
            && !group.manual_takeover
            && message_explicitly_mentions(&job.message, account_id)
        {
            if job.message.text.contains("预测") {
                match prediction::fetch(&job.message.text).await {
                    Ok(Ok(result)) => {
                        let _ = self
                                .enqueue_text_effect(
                                    account_id,
                                    group.group_id,
                                    &prediction::format_reply(&result),
                                    "prediction-reply",
                                    format!("prediction-reply:{}", job.message.id),
                                    serde_json::json!({"messageId":job.message.id,"userId":job.message.user_id}),
                                ).await;
                    }
                    Ok(Err(message)) => {
                        let _ = self.enqueue_text_effect(
                                account_id,
                                group.group_id,
                                &message,
                                "prediction-status",
                                format!("prediction-status:{}", job.message.id),
                            serde_json::json!({"messageId":job.message.id,"userId":job.message.user_id}),
                            ).await;
                    }
                    Err(error) => {
                        let _ = self.enqueue_text_effect(
                                account_id,
                                group.group_id,
                                &format!("预测数据暂时不可用：{}", error.message),
                                "prediction-error",
                                format!("prediction-error:{}", job.message.id),
                            serde_json::json!({"messageId":job.message.id,"userId":job.message.user_id}),
                            ).await;
                    }
                }
            } else {
                self.process_ai(app, account_id, &group, &member, &job.message)
                    .await?;
            }
        }
        let _ = self
            .database
            .record_audit(AuditEvent {
                id: 0,
                account_id: account_id.into(),
                group_id: job.message.group_id,
                user_id: job.message.user_id,
                actor: "DH BOT".into(),
                event: "message_processed".into(),
                level: "info".into(),
                details: format!("消息类型={}，序号={}", job.message.kind, job.sequence),
                created_at: Utc::now(),
            })
            .await;
        let _ = app.emit(
            "task-progress",
            RuntimeProgress {
                phase: "message".into(),
                detail: "消息已处理".into(),
                processed: 1,
            },
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_rule_actions(
        &self,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
        message: &Message,
        actions: &[RuleAction],
        rule_ids: &[i64],
        automatic: bool,
    ) {
        if !automatic {
            return;
        }
        let permission = self
            .gateway
            .list_members(message.group_id)
            .await
            .ok()
            .and_then(|roster| {
                roster
                    .members
                    .into_iter()
                    .find(|member| member.user_id == sender_id)
            })
            .map(|member| member.role == "owner" || member.role == "admin")
            .unwrap_or(false);
        let mut ordered = actions.iter().collect::<Vec<_>>();
        let mut contributor_rule_ids = rule_ids.to_vec();
        contributor_rule_ids.sort_unstable();
        contributor_rule_ids.dedup();
        let rule_id = contributor_rule_ids.first().copied();
        let contributor_key = contributor_rule_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join("-");
        ordered.sort_by_key(|action| match action.kind.as_str() {
            "recall" => 0,
            "remove" | "mute" | "unmute" => 1,
            "blacklist" => 2,
            "notify" | "reply" => 3,
            _ => 4,
        });
        for action in ordered {
            let dedupe_key = format!(
                "message:{}:rule:{}:action:{}",
                message.id,
                if contributor_key.is_empty() {
                    "ai"
                } else {
                    &contributor_key
                },
                action.kind
            );
            if self
                .database
                .action_succeeded(account_id.to_string(), dedupe_key.clone())
                .await
                .unwrap_or(false)
            {
                continue;
            }
            let mut error_text = String::new();
            if !permission {
                error_text = "需要将账号权限设置为管理".into();
            } else {
                let (effect_type, mut payload) = match action.kind.as_str() {
                    "recall" => (
                        "recall",
                        serde_json::json!({"serverMessageId":message.server_message_id}),
                    ),
                    "mute" => (
                        "mute",
                        serde_json::json!({"durationSeconds":action.duration_seconds}),
                    ),
                    "unmute" => ("unmute", serde_json::json!({})),
                    "remove" => ("remove", serde_json::json!({})),
                    "blacklist" => ("blacklist", serde_json::json!({})),
                    "notify" | "reply" => (
                        "send_text",
                        serde_json::json!({"text":action.message,"purpose":action.kind}),
                    ),
                    _ => {
                        error_text = "当前协议未开放该自动动作".into();
                        ("", Value::Null)
                    }
                };
                if error_text.is_empty() {
                    if let Some(object) = payload.as_object_mut() {
                        object.extend(
                            serde_json::json!({
                                "recordAction":true,
                                "actionKind":action.kind,
                                "userId":message.user_id,
                                "messageId":message.id,
                                "ruleId":rule_id,
                                "contributorRuleIds":contributor_rule_ids,
                                "mode":"automatic",
                                "durationSeconds":action.duration_seconds,
                                "reason":action.message,
                            })
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                        );
                    }
                    if let Err(error) = self
                        .enqueue_effect(
                            account_id,
                            message.group_id,
                            effect_type,
                            payload,
                            dedupe_key.clone(),
                        )
                        .await
                    {
                        error_text = error.message;
                    } else {
                        let _ = app.emit(
                            "action-recorded",
                            serde_json::json!({"kind":action.kind,"state":"queued"}),
                        );
                        continue;
                    }
                }
            }
            let _ = self
                .database
                .record_action(ActionRecord {
                    id: 0,
                    account_id: account_id.into(),
                    group_id: message.group_id,
                    user_id: message.user_id,
                    message_id: Some(message.id),
                    rule_id,
                    kind: action.kind.clone(),
                    mode: "auto".into(),
                    duration_seconds: action.duration_seconds,
                    reason: action.message.clone(),
                    success: false,
                    error: error_text.clone(),
                    receipt_json: String::new(),
                    dedupe_key,
                    created_at: Utc::now(),
                })
                .await;
            let _ = app.emit(
                "action-recorded",
                json_action(&action.kind, false, &error_text),
            );
        }
    }

    async fn classify_semantics(
        &self,
        group: &Group,
        member: &Member,
        message: &Message,
        categories: &[String],
    ) -> AppResult<HashMap<String, f64>> {
        let provider = ConfiguredProvider::new(AiConfig {
            base_url: self
                .database
                .get_setting("ai.base_url".into())
                .await?
                .unwrap_or_default(),
            webhook_url: self
                .database
                .get_setting("ai.webhook_url".into())
                .await?
                .unwrap_or_default(),
            model: self
                .database
                .get_setting("ai.model".into())
                .await?
                .unwrap_or_else(|| "deepseek-v4-pro".into()),
            api_key: self.gateway_secret("ai.api_key").await,
            timeout: Duration::from_secs(15),
        })?;
        let category_text = serde_json::to_string(categories)
            .map_err(|error| AppError::new("semantic_categories", error.to_string()))?;
        let request = AiRequest {
            version: "1",
            event_id: format!("semantic-{}", message.id),
            persona: "你是群消息语义分类器。只判断给定类别，不执行群动作。",
            group_id: group.group_id,
            group_name: group.name.clone(),
            member_id: member.user_id,
            member_name: member.card_name.clone(),
            member_role: member.role.clone(),
            message_id: message.server_message_id.clone(),
            message: format!(
                "类别={category_text}\n待分类消息={}\n按统一 AI 响应协议返回；reply 必须是字符串，字符串内容只包含一个 JSON 对象，键为类别，值为 0 到 1 的置信度。actions 和 tasks 必须为空。",
                message.text
            ),
            recent_context: Vec::new(),
            knowledge: Vec::new(),
        };
        let decision = provider.decide(&request).await?;
        parse_semantic_scores(&decision.reply, categories)
    }

    async fn process_ai(
        &self,
        app: &AppHandle,
        account_id: &str,
        group: &Group,
        member: &Member,
        message: &Message,
    ) -> AppResult<()> {
        let run_key = format!("message:{}", message.id);
        if !self
            .database
            .claim_ai_run(account_id.to_string(), group.group_id, run_key.clone())
            .await?
        {
            return Ok(());
        }
        let result: AppResult<()> = async {
            let base_url = self
                .database
                .get_setting("ai.base_url".into())
                .await?
                .unwrap_or_default();
            let webhook_url = self
                .database
                .get_setting("ai.webhook_url".into())
                .await?
                .unwrap_or_default();
            let model = self
                .database
                .get_setting("ai.model".into())
                .await?
                .unwrap_or_else(|| "deepseek-v4-pro".into());
            let api_key = self.gateway_secret("ai.api_key").await;
            let provider = ConfiguredProvider::new(AiConfig {
                base_url,
                webhook_url,
                model,
                api_key,
                timeout: Duration::from_secs(30),
            })?;
            let documents = self
                .database
                .list_knowledge_for_group(account_id.to_string(), group.group_id)
                .await?;
            let hits = knowledge::search(&message.text, &documents, 6);
            let recent_messages = self
                .database
                .recent_messages(account_id.to_string(), group.group_id, 21)
                .await?
                .into_iter()
                .filter(|candidate| candidate.id != message.id)
                .collect::<Vec<_>>();
            let recent_context = ai::build_recent_context(&recent_messages, 20);
            let request = AiRequest {
                version: "1",
                event_id: uuid::Uuid::new_v4().to_string(),
                persona: ai::PERSONA,
                group_id: group.group_id,
                group_name: group.name.clone(),
                member_id: member.user_id,
                member_name: member.card_name.clone(),
                member_role: member.role.clone(),
                message_id: message.server_message_id.clone(),
                message: ai::message_without_mention(&message.text),
                recent_context,
                knowledge: hits
                    .into_iter()
                    .map(|hit| ai::AiKnowledgeChunk {
                        base: hit.document.base_name,
                        title: hit.document.title,
                        source: hit.document.source,
                        text: hit.excerpt,
                    })
                    .collect(),
            };
            let decision = provider.decide(&request).await?;
            if self
                .ai_permission(account_id, group.group_id, "reply", group.ai_enabled)
                .await
                && !decision.reply.trim().is_empty()
            {
                self.enqueue_text_effect(
                    account_id,
                    group.group_id,
                    &decision.reply,
                    "ai-reply",
                    format!("ai-reply:{}", message.id),
                    serde_json::json!({"messageId":message.id,"userId":message.user_id}),
                )
                .await?;
            }
            let allow_recall = self
                .ai_permission(account_id, group.group_id, "recall", false)
                .await;
            let allow_mute = self
                .ai_permission(account_id, group.group_id, "mute", false)
                .await;
            let allow_remove = self
                .ai_permission(account_id, group.group_id, "remove", false)
                .await;
            let actions = decision
                .actions
                .into_iter()
                .filter(|action| match action.kind.as_str() {
                    "recall" => allow_recall,
                    "mute" | "unmute" => allow_mute,
                    "remove" => allow_remove,
                    _ => false,
                })
                .collect::<Vec<_>>();
            if !actions.is_empty() {
                self.execute_rule_actions(
                    app,
                    account_id,
                    self.gateway.session_identity().await?.0,
                    message,
                    &actions,
                    &[],
                    true,
                )
                .await;
            }
            if self
                .ai_permission(account_id, group.group_id, "tasks", true)
                .await
            {
                for (index, task) in decision.tasks.into_iter().enumerate() {
                    let _ = self
                        .database
                        .save_task_once(
                            TaskItem {
                                id: 0,
                                account_id: account_id.into(),
                                group_id: group.group_id,
                                title: task.title,
                                description: task.description,
                                status: "pending".into(),
                                assignee_id: 0,
                                created_by: self
                                    .gateway
                                    .session_identity()
                                    .await
                                    .map(|value| value.0)
                                    .unwrap_or(0),
                                due_at: task.due_at,
                                reminder_at: None,
                                reminder_sent_at: None,
                                created_at: Utc::now(),
                                updated_at: Utc::now(),
                            },
                            format!("ai-task:{}:{index}", message.id),
                        )
                        .await;
                }
            }
            Ok(())
        }
        .await;
        let _ = self
            .database
            .finish_ai_run(
                account_id.to_string(),
                run_key,
                result.is_ok(),
                result
                    .as_ref()
                    .err()
                    .map(|error| error.message.as_str())
                    .unwrap_or_default()
                    .to_string(),
            )
            .await;
        result
    }

    async fn ai_permission(
        &self,
        account_id: &str,
        group_id: i64,
        name: &str,
        default: bool,
    ) -> bool {
        if let Ok(Some(value)) = self
            .database
            .group_ai_permissions(account_id.to_string(), group_id)
            .await
        {
            return match name {
                "reply" => value.reply,
                "tasks" => value.tasks,
                "recall" => value.recall,
                "mute" => value.mute,
                "remove" => value.remove,
                _ => default,
            };
        }
        self.database
            .get_setting(format!("ai.permission.{name}.{account_id}.{group_id}"))
            .await
            .ok()
            .flatten()
            .map(|value| value == "true")
            .unwrap_or(default)
    }

    async fn gateway_secret(&self, key: &str) -> String {
        self.secrets
            .load()
            .ok()
            .and_then(|values| values.get(key).cloned())
            .unwrap_or_default()
    }
}

fn merge_managed_member_state(incoming: &mut Member, saved: &Member) {
    incoming.original_card_name = saved.original_card_name.clone();
    incoming.managed_card_name = saved.managed_card_name.clone();
    incoming.card_suffix = saved.card_suffix.clone();
    incoming.blacklisted = saved.blacklisted;
    incoming.prompt_read = saved.prompt_read;
    incoming.locked_card_name = saved.locked_card_name.clone();
    incoming.violation_count = saved.violation_count;
    incoming.discovered_at = saved.discovered_at;
    if incoming.joined_at.is_none() {
        incoming.joined_at = saved.joined_at;
    }
}

fn gateway_record_id(record: &GatewayRecord) -> String {
    if record.kind == GatewayRecordKind::Message {
        if let Some(id) = record
            .payload
            .get("idServer")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                record
                    .payload
                    .get("idClient")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
        {
            return format!("message:{id}");
        }
    }
    format!("{}:{}", record.session, record.sequence)
}

fn message_explicitly_mentions(message: &Message, account_id: &str) -> bool {
    if ai::is_mentioned(&message.text) {
        return true;
    }
    let Ok(metadata) = serde_json::from_str::<Value>(&message.mentions_json) else {
        return false;
    };
    fn contains_account(value: &Value, account_id: &str) -> bool {
        match value {
            Value::String(value) => value.eq_ignore_ascii_case(account_id),
            Value::Array(values) => values
                .iter()
                .any(|value| contains_account(value, account_id)),
            Value::Object(values) => values.iter().any(|(key, value)| {
                matches!(
                    key.as_str(),
                    "account" | "accid" | "nimId" | "accountId" | "target"
                ) && contains_account(value, account_id)
            }),
            _ => false,
        }
    }
    contains_account(&metadata, account_id)
}

fn parse_semantic_scores(reply: &str, categories: &[String]) -> AppResult<HashMap<String, f64>> {
    let trimmed = reply.trim();
    let without_prefix = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    let without_fence = without_prefix
        .strip_suffix("```")
        .unwrap_or(without_prefix)
        .trim();
    let start = without_fence
        .find('{')
        .ok_or_else(|| AppError::new("semantic_response", "语义分类结果缺少 JSON 对象"))?;
    let end = without_fence
        .rfind('}')
        .filter(|end| *end >= start)
        .ok_or_else(|| AppError::new("semantic_response", "语义分类结果缺少 JSON 对象"))?;
    let value: Value = serde_json::from_str(&without_fence[start..=end])
        .map_err(|error| AppError::new("semantic_response", error.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| AppError::new("semantic_response", "语义分类结果必须是 JSON 对象"))?;
    let mut scores = HashMap::with_capacity(categories.len());
    for category in categories {
        let score = object
            .get(category)
            .and_then(Value::as_f64)
            .filter(|score| score.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        scores.insert(category.clone(), score);
    }
    Ok(scores)
}

fn placeholder_member(account_id: &str, message: &Message) -> Member {
    Member {
        account_id: account_id.into(),
        group_id: message.group_id,
        user_id: message.user_id,
        nim_id: String::new(),
        nickname: message.sender_name.clone(),
        card_name: message.sender_name.clone(),
        original_card_name: message.sender_name.clone(),
        managed_card_name: String::new(),
        card_suffix: String::new(),
        role: "member".into(),
        account_state: String::new(),
        blacklisted: false,
        present: true,
        join_source: "message-discovered".into(),
        prompt_read: false,
        locked_card_name: String::new(),
        violation_count: 0,
        discovered_at: Utc::now(),
        joined_at: None,
        last_seen_at: Utc::now(),
        updated_at: Utc::now(),
    }
}
fn value_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}
fn timestamp(value: Option<i64>) -> Option<DateTime<Utc>> {
    let value = value?;
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
}
fn json_action(kind: &str, success: bool, error: &str) -> Value {
    serde_json::json!({"kind":kind,"success":success,"error":error})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::fixture::{FixtureGateway, FIXTURE_ACCOUNT, FIXTURE_GROUP};
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    fn test_paths() -> (tempfile::TempDir, AppPaths) {
        let directory = tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let paths = AppPaths {
            root: root.clone(),
            v3: root.join("3.0"),
            database: root.join("3.0/dh.db"),
            secrets: root.join("3.0/secrets.dat"),
            logs: root.join("3.0/logs"),
            legacy_backups: root.join("legacy-backups"),
            runtime_mode_file: root.join("runtime-mode"),
        };
        (directory, paths)
    }

    #[test]
    fn semantic_scores_accept_fenced_json_and_only_configured_categories() {
        let categories = vec!["广告".into(), "诈骗".into(), "辱骂".into()];
        let scores = parse_semantic_scores(
            "```json\n{\"广告\": 0.82, \"诈骗\": 1.4, \"extra\": 0.99}\n```",
            &categories,
        )
        .unwrap();
        assert_eq!(scores.len(), 3);
        assert_eq!(scores["广告"], 0.82);
        assert_eq!(scores["诈骗"], 1.0);
        assert_eq!(scores["辱骂"], 0.0);
        assert!(!scores.contains_key("extra"));
    }

    #[test]
    fn semantic_scores_reject_malformed_response() {
        let error = parse_semantic_scores("广告：高", &["广告".into()]).unwrap_err();
        assert_eq!(error.code, "semantic_response");
    }

    #[tokio::test]
    async fn durable_effect_outbox_executes_once_against_fixture_gateway() {
        let (_directory, paths) = test_paths();
        let database = Database::open(&paths).unwrap();
        let database = DatabaseExecutor::start(database).unwrap();
        let fixture = Arc::new(FixtureGateway::new_default());
        let runtime = BackendRuntime::new(
            database.clone(),
            fixture.clone(),
            SecretStore::new(&paths.secrets),
            Arc::new(Notify::new()),
            Logger::new(&paths.logs),
        );
        let payload = serde_json::json!({
            "text":"离群副作用测试",
            "recordAction":true,
            "actionKind":"reply",
            "userId":10006,
            "messageId":88,
            "mode":"automatic",
            "reason":"fixture-runtime",
        });
        for _ in 0..2 {
            runtime
                .enqueue_effect(
                    FIXTURE_ACCOUNT,
                    FIXTURE_GROUP,
                    "send_text",
                    payload.clone(),
                    "runtime-effect:88:reply".into(),
                )
                .await
                .unwrap();
        }
        let items = database
            .claim_effect_outbox(Some(FIXTURE_ACCOUNT.into()), 10)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        runtime.dispatch_effect(None, items[0].clone()).await;
        assert!(database
            .claim_effect_outbox(Some(FIXTURE_ACCOUNT.into()), 10)
            .await
            .unwrap()
            .is_empty());
        let snapshot = fixture.snapshot().await;
        assert_eq!(
            snapshot
                .actions
                .iter()
                .filter(|action| action.kind == "send_text")
                .count(),
            1
        );
        assert!(database
            .action_succeeded(FIXTURE_ACCOUNT.into(), "runtime-effect:88:reply".into())
            .await
            .unwrap());
        let (outbox_receipt, action_receipt, audit_details): (String, String, String) = database
            .execute(|database| database.with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT receipt_json FROM effect_outbox WHERE account_id=? AND dedupe_key=?",
                        rusqlite::params![FIXTURE_ACCOUNT, "runtime-effect:88:reply"],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT receipt_json FROM actions WHERE account_id=? AND dedupe_key=?",
                        rusqlite::params![FIXTURE_ACCOUNT, "runtime-effect:88:reply"],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT details FROM audit_events WHERE account_id=? AND event='effect_dispatched' ORDER BY id DESC LIMIT 1",
                        rusqlite::params![FIXTURE_ACCOUNT],
                        |row| row.get(0),
                    )?,
                ))
            }).map_err(crate::error::InternalError::from).map_err(AppError::from))
            .await
            .unwrap();
        for archived in [outbox_receipt, action_receipt, audit_details] {
            assert!(archived.contains("fixture-request-1"));
            assert!(archived.contains("succeeded"));
        }
        database.shutdown().await.unwrap();
    }
}
