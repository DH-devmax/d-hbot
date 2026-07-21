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
use crate::database::Database;
use crate::diagnostics::Logger;
use crate::error::{AppError, AppResult};
use crate::gateway::{ConnectionStatus, RuntimeGateway};
use crate::models::{
    Account, ActionRecord, AuditEvent, DailySummary, Group, Member, MemberRef, Message, RuleAction,
    TaskItem,
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
    database: Database,
    gateway: Arc<dyn RuntimeGateway>,
    secrets: SecretStore,
    shutdown: Arc<Notify>,
    logger: Logger,
}

impl BackendRuntime {
    pub fn new(
        database: Database,
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

    pub fn spawn(self, app: AppHandle) {
        let card_queue = self.clone();
        let card_app = app.clone();
        tauri::async_runtime::spawn(async move {
            card_queue.card_queue_loop(card_app).await;
        });
        let scheduler = self.clone();
        let scheduler_app = app.clone();
        tauri::async_runtime::spawn(async move {
            scheduler.schedule_loop(scheduler_app).await;
        });
        let connection = self.clone();
        let connection_app = app.clone();
        tauri::async_runtime::spawn(async move {
            connection.connection_loop(connection_app).await;
        });
        let reminders = self.clone();
        let reminders_app = app.clone();
        tauri::async_runtime::spawn(async move {
            reminders.reminder_loop(reminders_app).await;
        });
        let summaries = self.clone();
        let summaries_app = app.clone();
        tauri::async_runtime::spawn(async move {
            summaries.summary_loop(summaries_app).await;
        });
    }

    async fn summary_loop(&self, app: AppHandle) {
        let mut retry_after: HashMap<(String, i64, String), DateTime<Utc>> = HashMap::new();
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((_, account_id)) = self.gateway.session_identity().await {
                    let enabled = self
                        .database
                        .get_setting(&format!("summary.enabled.{account_id}"))
                        .ok()
                        .flatten()
                        .as_deref()
                        == Some("true");
                    let time = self
                        .database
                        .get_setting(&format!("summary.time.{account_id}"))
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "23:00".into());
                    let groups: Vec<i64> = self
                        .database
                        .get_setting(&format!("summary.groups.{account_id}"))
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
                            .list_daily_summaries(&account_id, 100)
                            .unwrap_or_default();
                        for group_id in groups.into_iter().filter(|group_id| {
                            !existing.iter().any(|summary| {
                                summary.group_id == *group_id && summary.local_date == local_date
                            })
                        }) {
                            let key = (account_id.clone(), group_id, local_date.clone());
                            if retry_after
                                .get(&key)
                                .is_some_and(|value| *value > Utc::now())
                            {
                                continue;
                            }
                            match self
                                .generate_scheduled_summary(&account_id, group_id, &local_date)
                                .await
                            {
                                Ok(summary) => {
                                    retry_after.remove(&key);
                                    let _ = app.emit("task-progress", serde_json::json!({"kind":"dailySummary","groupId":group_id,"summaryId":summary.id,"success":true}));
                                }
                                Err(error) => {
                                    retry_after
                                        .insert(key, Utc::now() + chrono::Duration::minutes(10));
                                    let _ = self.database.record_audit(&AuditEvent {
                                        id: 0,
                                        account_id: account_id.clone(),
                                        group_id,
                                        user_id: 0,
                                        actor: "DH BOT".into(),
                                        event: "daily_summary_failed".into(),
                                        level: "error".into(),
                                        details: error.message.clone(),
                                        created_at: Utc::now(),
                                    });
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
            .list_messages(account_id, Some(group_id), 1000)?
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
                .get_setting("ai.base_url")?
                .unwrap_or_default(),
            webhook_url: self
                .database
                .get_setting("ai.webhook_url")?
                .unwrap_or_default(),
            model: self
                .database
                .get_setting("ai.model")?
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
        let id = self.database.save_daily_summary(&summary)?;
        Ok(DailySummary { id, ..summary })
    }

    async fn reminder_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok(tasks) = self.database.claim_due_task_reminders(Utc::now(), 20) {
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
                        let result = self.gateway.send_text(task.group_id, &text).await;
                        let (level, details, success) = match result {
                            Ok(_) => ("info", format!("任务“{}”提醒已发送", task.title), true),
                            Err(error) => (
                                "error",
                                format!("任务“{}”提醒发送失败：{}", task.title, error.message),
                                false,
                            ),
                        };
                        let _ = self.database.record_audit(&AuditEvent {
                            id: 0,
                            account_id: task.account_id.clone(),
                            group_id: task.group_id,
                            user_id: task.assignee_id,
                            actor: "DH BOT".into(),
                            event: "task_reminder".into(),
                            level: level.into(),
                            details: details.clone(),
                            created_at: Utc::now(),
                        });
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
                .get_setting("automation.mode")
                .ok()
                .flatten()
                .as_deref()
                == Some("paused");
            if !globally_paused && self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((sender_id, account_id)) = self.gateway.session_identity().await {
                    if let Ok(Some(job)) = self.database.claim_next_card_job(&account_id) {
                        let paused = self
                            .database
                            .get_setting(&format!(
                                "card.paused.{}.{}",
                                job.account_id, job.group_id
                            ))
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
                        let _ = self.database.finish_card_job(&job, success, &error);
                        if success && job.welcome_pending {
                            let welcome = self
                                .database
                                .list_groups(Some(&job.account_id))
                                .ok()
                                .and_then(|groups| {
                                    groups
                                        .into_iter()
                                        .find(|group| group.group_id == job.group_id)
                                })
                                .map(|group| group.welcome_message)
                                .unwrap_or_default();
                            if welcome.trim().is_empty()
                                || self
                                    .gateway
                                    .send_text(
                                        job.group_id,
                                        &welcome.replace("[成员]", &job.desired_name),
                                    )
                                    .await
                                    .is_ok()
                            {
                                let _ = self.database.mark_card_welcome_sent(job.id);
                            }
                        }
                        let _ = self.database.record_audit(&AuditEvent {
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
                        });
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
                .list_members(account_id, group.group_id)
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
                let _ = self.database.upsert_member(&member);
                if !baseline && newly_discovered.contains(&member.user_id) && member.blacklisted {
                    let removed = self
                        .gateway
                        .remove_member(group.group_id, member.user_id)
                        .await
                        .is_ok();
                    let _ = self.database.record_audit(&AuditEvent {
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
                    });
                }
            }
            let automatic = self
                .database
                .get_setting(&format!("card.auto.{account_id}.{}", group.group_id))
                .ok()
                .flatten()
                .as_deref()
                == Some("true");
            if !baseline && automatic && !newly_discovered.is_empty() {
                let prefix = self
                    .database
                    .get_setting(&format!("card.prefix.{account_id}.{}", group.group_id))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "DH".into());
                if let Ok(preview) = crate::cardnames::preview(
                    group.group_id,
                    &prefix,
                    self.database
                        .list_members(account_id, group.group_id)
                        .unwrap_or_default(),
                    sender_id,
                ) {
                    for plan in preview.items.into_iter().filter(|plan| {
                        plan.status == "planned" && newly_discovered.contains(&plan.member.user_id)
                    }) {
                        let _ = self.database.enqueue_card_job(
                            account_id,
                            group.group_id,
                            &plan,
                            false,
                        );
                    }
                }
            }
            if roster.complete && !baseline {
                let missing = self
                    .database
                    .mark_members_not_present(
                        account_id,
                        group.group_id,
                        &present_ids.iter().copied().collect::<Vec<_>>(),
                    )
                    .unwrap_or_default();
                for member in missing {
                    let _ = self.database.record_audit(&AuditEvent {
                        id: 0,
                        account_id: account_id.into(),
                        group_id: group.group_id,
                        user_id: member.user_id,
                        actor: "DH BOT".into(),
                        event: "member_left".into(),
                        level: "info".into(),
                        details: "成员同步确认已离群".into(),
                        created_at: Utc::now(),
                    });
                }
            }
        }
    }

    async fn schedule_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((sender_id, account_id)) = self.gateway.session_identity().await {
                    if let Ok(schedules) = self.database.list_schedules(&account_id) {
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
                                        &account_id,
                                        *group_id,
                                        &decision.last_action,
                                        &decision.run_key,
                                    )
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
                                let result = if allowed {
                                    self.gateway
                                        .set_group_mute(*group_id, decision.should_mute)
                                        .await
                                } else {
                                    Err(crate::error::AppError::new(
                                        "management_required",
                                        "需要将账号权限设置为管理",
                                    ))
                                };
                                let (success, error) = match result {
                                    Ok(()) => (true, String::new()),
                                    Err(error) => (false, error.message),
                                };
                                let _ = self.database.finish_schedule_run(
                                    &decision.run_key,
                                    success,
                                    &error,
                                );
                                let _ = self.database.record_audit(&AuditEvent {
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
                                        format!("计划“{}”已执行", schedule.name)
                                    } else {
                                        error.clone()
                                    },
                                    created_at: Utc::now(),
                                });
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
            let _ = self.database.upsert_account(&Account {
                id: account_id.clone(),
                display_name: diagnostic.nim_account.clone(),
                role: "unknown".into(),
                discovered_at: now,
                updated_at: now,
            });
            if let Ok(groups) = self.gateway.list_groups().await {
                for group in groups {
                    let _ = self.database.upsert_group(&group);
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
            if let Ok(pending) = self.database.list_pending_messages(&account_id, 1000) {
                for message in pending {
                    let sender = workers
                        .entry(message.group_id)
                        .or_insert_with(|| {
                            let (sender, mut receiver) = mpsc::channel::<IncomingJob>(128);
                            let runtime = self.clone();
                            let app_handle = app.clone();
                            let account = account_id.clone();
                            tokio::spawn(async move {
                                while let Some(job) = receiver.recv().await {
                                    if runtime
                                        .database
                                        .claim_message(job.message.id)
                                        .unwrap_or(false)
                                    {
                                        let result = runtime
                                            .process_job(
                                                &app_handle,
                                                &account,
                                                sender_id,
                                                job.clone(),
                                            )
                                            .await;
                                        let (success, error) = match result {
                                            Ok(()) => (true, String::new()),
                                            Err(error) => (false, error.message),
                                        };
                                        let _ = runtime.database.finish_message_processing(
                                            job.message.id,
                                            success,
                                            &error,
                                        );
                                    }
                                }
                            });
                            sender
                        })
                        .clone();
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
                let batch = match self.gateway.poll_messages().await {
                    Ok(value) => value,
                    Err(error) => {
                        self.logger.write("WARN", &error.message);
                        let _ = app.emit("connection-error", &error);
                        break;
                    }
                };
                let messages = batch
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for value in messages {
                    if let Some(mut job) = self.normalize_message(&account_id, &value) {
                        let persisted = match self.database.insert_message(&job.message) {
                            Ok(value) => value,
                            Err(error) => {
                                self.logger.write("WARN", &error.message);
                                continue;
                            }
                        };
                        if self
                            .gateway
                            .acknowledge_messages(job.sequence)
                            .await
                            .is_err()
                        {
                            continue;
                        }
                        let _ = self.database.mark_message_acknowledged(persisted.id);
                        if persisted.processed {
                            continue;
                        }
                        job.message.id = persisted.id;
                        let sender = workers
                            .entry(job.message.group_id)
                            .or_insert_with(|| {
                                let (sender, mut receiver) = mpsc::channel::<IncomingJob>(128);
                                let runtime = self.clone();
                                let app_handle = app.clone();
                                let account = account_id.clone();
                                tokio::spawn(async move {
                                    while let Some(job) = receiver.recv().await {
                                        if runtime
                                            .database
                                            .claim_message(job.message.id)
                                            .unwrap_or(false)
                                        {
                                            let result = runtime
                                                .process_job(
                                                    &app_handle,
                                                    &account,
                                                    sender_id,
                                                    job.clone(),
                                                )
                                                .await;
                                            let (success, error) = match result {
                                                Ok(()) => (true, String::new()),
                                                Err(error) => (false, error.message),
                                            };
                                            let _ = runtime.database.finish_message_processing(
                                                job.message.id,
                                                success,
                                                &error,
                                            );
                                        }
                                    }
                                });
                                sender
                            })
                            .clone();
                        let _ = sender.send(job).await;
                        let _ = app.emit("message-received", &value);
                    }
                }
                tokio::select! { _ = sleep(Duration::from_millis(700)) => {}, _ = self.shutdown.notified() => return }
            }
            tokio::select! { _ = sleep(Duration::from_secs(5)) => {}, _ = self.shutdown.notified() => break }
        }
    }

    fn normalize_message(&self, account_id: &str, value: &Value) -> Option<IncomingJob> {
        let sequence = value.get("seq").and_then(Value::as_u64)?;
        let decoded = value.get("decoded").unwrap_or(&Value::Null);
        let group_id = decoded
            .pointer("/to/id")
            .and_then(value_i64)
            .or_else(|| value.get("to").and_then(value_i64))?;
        if group_id <= 0 {
            return None;
        }
        let user_id = decoded
            .pointer("/from/id")
            .and_then(value_i64)
            .or_else(|| value.get("from").and_then(value_i64))
            .unwrap_or(0);
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
        Some(IncomingJob {
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
            .get_setting("automation.mode")
            .ok()
            .flatten()
            .as_deref()
            == Some("paused")
        {
            let _ = self.database.record_audit(&AuditEvent {
                id: 0,
                account_id: account_id.into(),
                group_id: job.message.group_id,
                user_id: job.message.user_id,
                actor: "DH BOT".into(),
                event: "automation_paused".into(),
                level: "info".into(),
                details: "托盘暂停了自动化执行".into(),
                created_at: Utc::now(),
            });
            return Ok(());
        }
        let Some(group) = self
            .database
            .list_groups(Some(account_id))
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
            .list_members(account_id, group.group_id)
            .ok()
            .and_then(|members| {
                members
                    .into_iter()
                    .find(|member| member.user_id == job.message.user_id)
            })
            .unwrap_or_else(|| {
                let member = placeholder_member(account_id, &job.message);
                let _ = self.database.upsert_member(&member);
                member
            });
        let recent_messages = self
            .database
            .recent_messages(account_id, group.group_id, 20)
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
            if let Ok(rules) = self.database.list_rules(account_id, Some(group.group_id)) {
                let mut decision = moderation::evaluate(
                    &rules,
                    &moderation::ModerationInput {
                        member: &member,
                        kind: &job.message.kind,
                        text: &job.message.text,
                        now: Utc::now(),
                        recent: &recent_events,
                        rename_violations: member.violation_count,
                    },
                );
                let mut executable = Vec::new();
                let mut first_rule_id = None;
                for matched in &decision.matches {
                    if matched.mode == "auto" {
                        let cooldown = rules
                            .iter()
                            .find(|rule| rule.id == matched.rule_id)
                            .map(|rule| rule.cooldown_seconds)
                            .unwrap_or(0);
                        let allowed = cooldown <= 0
                            || self
                                .database
                                .record_rule_runtime(
                                    matched.rule_id,
                                    account_id,
                                    group.group_id,
                                    member.user_id,
                                    Utc::now(),
                                    cooldown,
                                )
                                .unwrap_or(false);
                        if allowed {
                            first_rule_id.get_or_insert(matched.rule_id);
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
                    first_rule_id,
                    decision.automatic,
                )
                .await;
            }
        }
        if group.enabled && !group.manual_takeover && ai::is_mentioned(&job.message.text) {
            if prediction::is_prediction_request(&job.message.text) {
                if group.ai_enabled {
                    match prediction::fetch(&job.message.text).await {
                        Ok(Ok(result)) => {
                            let _ = self
                                .gateway
                                .send_text(group.group_id, &prediction::format_reply(&result))
                                .await;
                        }
                        Ok(Err(message)) => {
                            let _ = self.gateway.send_text(group.group_id, &message).await;
                        }
                        Err(error) => {
                            let _ = self
                                .gateway
                                .send_text(
                                    group.group_id,
                                    &format!("预测数据暂时不可用：{}", error.message),
                                )
                                .await;
                        }
                    }
                }
            } else {
                self.process_ai(app, account_id, &group, &member, &job.message)
                    .await?;
            }
        }
        let _ = self.database.record_audit(&AuditEvent {
            id: 0,
            account_id: account_id.into(),
            group_id: job.message.group_id,
            user_id: job.message.user_id,
            actor: "DH BOT".into(),
            event: "message_processed".into(),
            level: "info".into(),
            details: format!("消息类型={}，序号={}", job.message.kind, job.sequence),
            created_at: Utc::now(),
        });
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
        rule_id: Option<i64>,
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
        for action in actions {
            let dedupe_key = format!(
                "message:{}:rule:{}:action:{}",
                message.id,
                rule_id.unwrap_or(0),
                action.kind
            );
            if self
                .database
                .action_succeeded(account_id, &dedupe_key)
                .unwrap_or(false)
            {
                continue;
            }
            let mut success = false;
            let mut error_text = String::new();
            if !permission {
                error_text = "需要将账号权限设置为管理".into();
            } else {
                let result = match action.kind.as_str() {
                    "recall" => {
                        self.gateway
                            .recall(
                                message.group_id,
                                message.user_id,
                                &message.server_message_id,
                            )
                            .await
                    }
                    "mute" => {
                        self.gateway
                            .mute(message.group_id, message.user_id, action.duration_seconds)
                            .await
                    }
                    "unmute" => self.gateway.unmute(message.group_id, message.user_id).await,
                    "remove" => {
                        self.gateway
                            .remove_member(message.group_id, message.user_id)
                            .await
                    }
                    "blacklist" => self.database.set_member_blacklisted(
                        account_id,
                        message.group_id,
                        message.user_id,
                        true,
                    ),
                    "notify" => self
                        .gateway
                        .send_text(message.group_id, &action.message)
                        .await
                        .map(|_| ()),
                    "reply" => self
                        .gateway
                        .send_text(message.group_id, &action.message)
                        .await
                        .map(|_| ()),
                    _ => Err(AppError::new(
                        "action_unsupported",
                        "当前协议未开放该自动动作",
                    )),
                };
                if let Err(error) = result {
                    error_text = error.message;
                } else {
                    success = true;
                }
            }
            let _ = self.database.record_action(&ActionRecord {
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
                success,
                error: error_text.clone(),
                dedupe_key,
                created_at: Utc::now(),
            });
            let _ = app.emit(
                "action-recorded",
                json_action(&action.kind, success, &error_text),
            );
        }
    }

    async fn process_ai(
        &self,
        app: &AppHandle,
        account_id: &str,
        group: &Group,
        member: &Member,
        message: &Message,
    ) -> AppResult<()> {
        let base_url = self
            .database
            .get_setting("ai.base_url")?
            .unwrap_or_default();
        let webhook_url = self
            .database
            .get_setting("ai.webhook_url")?
            .unwrap_or_default();
        let model = self
            .database
            .get_setting("ai.model")?
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
            .list_knowledge_for_group(account_id, group.group_id)?;
        let hits = knowledge::search(&message.text, &documents, 6);
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
            message: message.text.clone(),
            recent_context: Vec::new(),
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
        if group.ai_enabled && !decision.reply.trim().is_empty() {
            let _ = self
                .gateway
                .send_text(group.group_id, &decision.reply)
                .await;
        }
        let actions = decision
            .actions
            .into_iter()
            .filter(|action| match action.kind.as_str() {
                "recall" => self.ai_permission(account_id, group.group_id, "recall", false),
                "mute" | "unmute" => self.ai_permission(account_id, group.group_id, "mute", false),
                "remove" => self.ai_permission(account_id, group.group_id, "remove", false),
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
                None,
                true,
            )
            .await;
        }
        if self.ai_permission(account_id, group.group_id, "tasks", true) {
            for task in decision.tasks {
                let _ = self.database.save_task(&TaskItem {
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
                });
            }
        }
        Ok(())
    }

    fn ai_permission(&self, account_id: &str, group_id: i64, name: &str, default: bool) -> bool {
        if let Ok(Some(value)) = self.database.group_ai_permissions(account_id, group_id) {
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
            .get_setting(&format!("ai.permission.{name}.{account_id}.{group_id}"))
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
