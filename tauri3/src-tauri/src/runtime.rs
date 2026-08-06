use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::sleep;

use crate::ai::{
    self, AiConfig, AiContextMessage, AiKnowledgeCache, AiProvider, AiProviderPool, AiReplyCache,
    AiRequest,
};
use crate::business_apps::{BusinessApp, BusinessAppContext, BusinessAppRegistry};
use crate::database::DatabaseExecutor;
use crate::diagnostics::{redact, Logger};
use crate::error::{AppError, AppResult};
use crate::gateway::{
    synthetic_nim_user_id, AutomaticWritePermit, CapabilityStatus, ConnectionStatus, GatewayEvent,
    GatewayReceipt, GatewayRecord, GatewayRecordKind, RuntimeGateway,
};
use crate::models::{
    Account, ActionRecord, Activity, AuditEvent, DailySummary, EffectOutboxItem,
    EffectOutboxRequest, GatewayInboxEvent, Group, Member, MemberRef, Message, ModerationRule,
    RuleAction,
};
use crate::runtime_work::RuntimeCoordination;
use crate::secrets::SecretStore;
use crate::shutdown::ShutdownSignal;
use crate::{knowledge, moderation, prediction, scheduler};

/// Hard cap for the per-connection member-event cache inside `connection_loop`.
const MAX_MEMBER_EVENT_CACHE: usize = 500;
/// Hard cap for the dedup warning sets (`reported_*`) inside `connection_loop`.
const MAX_REPORTED_SET: usize = 500;

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

#[derive(Debug, Clone)]
struct AiReplyJob {
    message: Message,
}

struct AiRuleJob {
    account_id: String,
    sender_id: i64,
    group: Group,
    member: Member,
    message: Message,
    recent_events: Vec<moderation::RecentEvent>,
    rules: Vec<ModerationRule>,
}

struct PredictionNarrationJob {
    account_id: String,
    group_id: i64,
    cache_key: String,
    narration: Value,
    provider: Arc<dyn AiProvider>,
    request: AiRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectDispatchStatus {
    Succeeded,
    Failed,
    TerminalFailed,
    Unknown,
}

fn classify_effect_error(error: &AppError) -> EffectDispatchStatus {
    if error.delivery_outcome_unknown() {
        EffectDispatchStatus::Unknown
    } else if !error.retryable
        || error.gateway.as_deref().is_some_and(|metadata| {
            metadata.business_code == Some(1001) || metadata.business_errno == Some(1001)
        })
    {
        EffectDispatchStatus::TerminalFailed
    } else {
        EffectDispatchStatus::Failed
    }
}

fn next_connection_retry(delay: Duration) -> Duration {
    Duration::from_millis(
        (delay.as_millis() as u64)
            .saturating_mul(2)
            .clamp(250, 2_000),
    )
}

/// 会话至少稳定运行这么久，才认为上一轮连接是健康的，可以把重连退避清零。
/// 否则连续快速断开（监听会话切换、队列溢出、序列缺口）会一直以最短间隔重连。
const HEALTHY_SESSION_THRESHOLD: Duration = Duration::from_secs(30);

pub(crate) fn runtime_lane_id(kind: &str, account_id: &str, group_id: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(account_id.as_bytes());
    hasher.update(group_id.to_le_bytes());
    format!("{}-{}", kind, &format!("{:x}", hasher.finalize())[..12])
}

fn effect_label(effect_type: &str) -> &'static str {
    match effect_type {
        "send_text" => "发送群消息",
        "recall" => "撤回群消息",
        "mute" | "unmute" => "更新成员禁言",
        "rename" => "修改群名片",
        "group_mute" => "更新群发言状态",
        "remove" => "移出群成员",
        _ => "执行群操作",
    }
}

pub trait Clock: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
    fn now_local(&self) -> DateTime<Local>;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn now_local(&self) -> DateTime<Local> {
        Local::now()
    }
}

impl crate::queue_kernel::ClockSource for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

#[derive(Default)]
pub struct NoopEventSink;

impl RuntimeEventSink for NoopEventSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl RuntimeEventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.app.emit(event, payload);
    }
}

pub trait AiProviderFactory: Send + Sync {
    fn create(&self, configs: Vec<AiConfig>) -> AppResult<Arc<dyn AiProvider>>;
}

#[derive(Clone, Default)]
pub struct ConfiguredAiProviderFactory {
    pool: AiProviderPool,
}

impl AiProviderFactory for ConfiguredAiProviderFactory {
    fn create(&self, configs: Vec<AiConfig>) -> AppResult<Arc<dyn AiProvider>> {
        self.pool.chain(configs)
    }
}

impl ConfiguredAiProviderFactory {
    pub fn new(pool: AiProviderPool) -> Self {
        Self { pool }
    }
}

pub struct RuntimeDependencies {
    pub clock: Arc<dyn Clock>,
    pub events: Arc<dyn RuntimeEventSink>,
    pub ai_factory: Arc<dyn AiProviderFactory>,
    pub semantic_classifier: Arc<dyn moderation::SemanticClassifier>,
    pub prediction_source: Arc<dyn prediction::PredictionSource>,
}

impl RuntimeDependencies {
    pub fn production() -> AppResult<Self> {
        Ok(Self {
            clock: Arc::new(SystemClock),
            events: Arc::new(NoopEventSink),
            ai_factory: Arc::new(ConfiguredAiProviderFactory::default()),
            semantic_classifier: Arc::new(moderation::DeterministicSemanticClassifier::default()),
            prediction_source: Arc::new(prediction::PublicLotterySource::new(
                Duration::from_secs(8),
            )?),
        })
    }
}

impl EffectDispatchStatus {
    fn succeeded(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

fn capability_for_effect(
    capabilities: &crate::gateway::GatewayCapabilities,
    effect_type: &str,
) -> Option<crate::gateway::GatewayCapability> {
    match effect_type {
        "send_text" => Some(capabilities.send_text.clone()),
        "recall" => Some(capabilities.recall.clone()),
        "mute" | "unmute" => Some(capabilities.mute.clone()),
        "remove" => Some(capabilities.remove_member.clone()),
        "rename" => Some(capabilities.rename.clone()),
        "group_mute" => Some(capabilities.group_mute.clone()),
        _ => None,
    }
}

#[derive(Clone)]
pub struct BackendRuntime {
    database: DatabaseExecutor,
    gateway: Arc<dyn RuntimeGateway>,
    secrets: SecretStore,
    shutdown: Arc<ShutdownSignal>,
    logger: Logger,
    clock: Arc<dyn Clock>,
    events: Arc<dyn RuntimeEventSink>,
    ai_factory: Arc<dyn AiProviderFactory>,
    semantic_classifier: Arc<dyn moderation::SemanticClassifier>,
    business_apps: Arc<BusinessAppRegistry>,
    ai_reply_cache: AiReplyCache,
    ai_knowledge_cache: AiKnowledgeCache,
    prediction_narration_cache: AiReplyCache,
    prediction_narration_tx: mpsc::Sender<PredictionNarrationJob>,
    prediction_narration_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<PredictionNarrationJob>>>,
    prediction_narration_pending: Arc<tokio::sync::Mutex<HashSet<String>>>,
    ai_rule_gate: Arc<tokio::sync::Semaphore>,
    coordination: RuntimeCoordination,
    queue_kernel: Arc<crate::queue_kernel::QueueKernel>,
}

impl BackendRuntime {
    pub fn new(
        database: DatabaseExecutor,
        gateway: Arc<dyn RuntimeGateway>,
        secrets: SecretStore,
        shutdown: Arc<ShutdownSignal>,
        logger: Logger,
    ) -> Self {
        let dependencies = RuntimeDependencies::production()
            .expect("production runtime dependencies must initialize");
        Self::with_dependencies(database, gateway, secrets, shutdown, logger, dependencies)
    }

    pub fn new_with_ai_pool(
        database: DatabaseExecutor,
        gateway: Arc<dyn RuntimeGateway>,
        secrets: SecretStore,
        shutdown: Arc<ShutdownSignal>,
        logger: Logger,
        pool: AiProviderPool,
        prediction_source: Arc<dyn prediction::PredictionSource>,
    ) -> Self {
        let mut dependencies = RuntimeDependencies::production()
            .expect("production runtime dependencies must initialize");
        dependencies.ai_factory = Arc::new(ConfiguredAiProviderFactory::new(pool));
        dependencies.prediction_source = prediction_source;
        Self::with_dependencies(database, gateway, secrets, shutdown, logger, dependencies)
    }

    pub fn with_dependencies(
        database: DatabaseExecutor,
        gateway: Arc<dyn RuntimeGateway>,
        secrets: SecretStore,
        shutdown: Arc<ShutdownSignal>,
        logger: Logger,
        dependencies: RuntimeDependencies,
    ) -> Self {
        let (prediction_narration_tx, prediction_narration_rx) = mpsc::channel(64);
        Self {
            database,
            gateway,
            secrets,
            shutdown,
            logger,
            clock: dependencies.clock,
            events: dependencies.events,
            ai_factory: dependencies.ai_factory,
            semantic_classifier: dependencies.semantic_classifier,
            business_apps: Arc::new(BusinessAppRegistry::new(dependencies.prediction_source)),
            ai_reply_cache: AiReplyCache::default(),
            ai_knowledge_cache: AiKnowledgeCache::default(),
            prediction_narration_cache: AiReplyCache::new(512, Duration::from_secs(30 * 60)),
            prediction_narration_tx,
            prediction_narration_rx: Arc::new(tokio::sync::Mutex::new(prediction_narration_rx)),
            prediction_narration_pending: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            ai_rule_gate: Arc::new(tokio::sync::Semaphore::new(2)),
            coordination: RuntimeCoordination::default(),
            queue_kernel: Arc::new(crate::queue_kernel::QueueKernel::new(Arc::new(
                SystemClock,
            ))),
        }
    }

    pub fn with_coordination(mut self, coordination: RuntimeCoordination) -> Self {
        self.coordination = coordination;
        self
    }

    pub fn spawn(mut self, app: AppHandle) -> Vec<tauri::async_runtime::JoinHandle<()>> {
        self.events = Arc::new(TauriEventSink::new(app.clone()));
        self.coordination.tracker.attach(app.clone());
        let mut workers = Vec::with_capacity(9);
        let backlog = self.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            backlog.seed_runtime_work().await;
        }));
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
        let roster_sync = self.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            roster_sync.roster_sync_loop().await;
        }));
        let activities = self.clone();
        let activities_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            activities.activity_loop(activities_app).await;
        }));
        let summaries = self.clone();
        let summaries_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            summaries.summary_loop(summaries_app).await;
        }));
        let effects = self.clone();
        let effects_app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            effects.effect_dispatcher_loop(effects_app).await;
        }));
        let prediction_narration = self.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            prediction_narration.prediction_narration_loop().await;
        }));
        workers
    }

    async fn seed_runtime_work(&self) {
        let Ok(items) = self.database.runtime_backlog().await else {
            return;
        };
        for item in items {
            let (id_kind, kind, label, scope) = match item.lane.as_str() {
                "message" => ("message", "message", "处理群消息", "群消息队列"),
                "cardRename" => ("cardRename", "cardRename", "批量修改群名片", "群名片队列"),
                "write" => (
                    item.detail.as_str(),
                    "write",
                    effect_label(&item.detail),
                    "群操作",
                ),
                _ => continue,
            };
            let work_id = runtime_lane_id(id_kind, &item.account_id, item.group_id);
            self.coordination
                .tracker
                .seed_queued_count(&work_id, kind, label, scope, item.count);
        }
        self.coordination.notify();
    }

    async fn prediction_narration_loop(&self) {
        let mut receiver = self.prediction_narration_rx.lock().await;
        loop {
            let job = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                job = receiver.recv() => match job {
                    Some(job) => job,
                    None => break,
                },
            };
            let work_id = runtime_lane_id("prediction", &job.account_id, job.group_id);
            let Ok(_permit) = self.ai_rule_gate.clone().acquire_owned().await else {
                self.coordination.tracker.start_named(
                    &work_id,
                    "prediction",
                    "生成预测说明",
                    "业务应用",
                    None,
                );
                self.coordination
                    .tracker
                    .finish(&work_id, "failed", "AI 调度器已停止");
                break;
            };
            self.coordination.tracker.start_named(
                &work_id,
                "prediction",
                "生成预测说明",
                "业务应用",
                None,
            );
            let outcome = match job.provider.decide(&job.request).await {
                Ok(decision)
                    if crate::business_apps::validate_prediction_narration(
                        &decision.reply,
                        &job.narration,
                    ) =>
                {
                    self.prediction_narration_cache.insert_scoped(
                        &job.account_id,
                        job.cache_key.clone(),
                        decision,
                    );
                    Ok(())
                }
                Ok(_) => Err("AI 预测说明格式不完整"),
                Err(_) => Err("AI 预测说明生成失败"),
            };
            match outcome {
                Ok(()) => self.coordination.tracker.finish(&work_id, "succeeded", ""),
                Err(error) => self.coordination.tracker.finish(&work_id, "failed", error),
            }
            self.prediction_narration_pending
                .lock()
                .await
                .remove(&job.cache_key);
        }
    }

    async fn enqueue_effect(
        &self,
        account_id: &str,
        group_id: i64,
        effect_type: &str,
        payload: Value,
        dedupe_key: String,
    ) -> AppResult<()> {
        if self.gateway.calibration_active() {
            return Err(AppError::new(
                "calibration_active",
                "真实校准期间已暂停自动副作用，避免后台任务修改测试群状态",
            ));
        }
        let enqueued = self
            .database
            .enqueue_effect(EffectOutboxRequest {
                account_id: account_id.into(),
                group_id,
                effect_type: effect_type.into(),
                payload_json: payload.to_string(),
                dedupe_key,
            })
            .await?;
        if enqueued.inserted && matches!(enqueued.state.as_str(), "queued" | "retry") {
            let work_id = runtime_lane_id(effect_type, account_id, group_id);
            self.coordination.tracker.enqueue(
                &work_id,
                "write",
                effect_label(effect_type),
                "群操作",
            );
            self.coordination.notify();
        }
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

    /// Batch dispatcher. Replaces the old single-item effect_loop.
    ///
    /// Claims up to 4 items per tick, runs each through QueueKernel
    /// (expiry guard → order-key lock → lane concurrency gate), then:
    ///   • Skip   → mark succeeded, no dispatch
    ///   • Fail   → mark failed-terminal immediately
    ///   • Retry  → mark failed, let backoff reschedule
    ///   • Proceed → spawn a task holding the concurrency guards and dispatch
    async fn effect_dispatcher_loop(&self, app: AppHandle) {
        loop {
            let mut dispatched: usize = 0;
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((_, account_id)) = self.gateway.session_identity().await {
                    if let Ok(items) = self
                        .database
                        .claim_effect_outbox(Some(account_id.clone()), 4)
                        .await
                    {
                        for item in items {
                            use crate::queue_kernel::ChainOutcome;
                            let outcome = self
                                .queue_kernel
                                .run_chain(item.clone(), account_id.clone())
                                .await;
                            match outcome {
                                ChainOutcome::Skip { reason } => {
                                    self.skip_effect_item(&item, &reason).await;
                                    dispatched += 1;
                                }
                                ChainOutcome::Fail { error } => {
                                    self.reject_effect_item(&item, &error, false).await;
                                    dispatched += 1;
                                }
                                ChainOutcome::Retry { error } => {
                                    self.reject_effect_item(&item, &error, true).await;
                                    dispatched += 1;
                                }
                                ChainOutcome::Proceed {
                                    order_guard,
                                    lane_permit,
                                } => {
                                    let work_id = runtime_lane_id(
                                        &item.effect_type,
                                        &item.account_id,
                                        item.group_id,
                                    );
                                    self.coordination.tracker.start_named(
                                        &work_id,
                                        "write",
                                        effect_label(&item.effect_type),
                                        "群操作",
                                        None,
                                    );
                                    match self.gateway.automatic_write_permit().await {
                                        Ok(permit) => {
                                            let rt = self.clone();
                                            let app_clone = app.clone();
                                            tauri::async_runtime::spawn(async move {
                                                rt.dispatch_effect_permitted(
                                                    Some(&app_clone),
                                                    item,
                                                    &permit,
                                                )
                                                .await;
                                                // Release order-key and lane slots
                                                drop(order_guard);
                                                drop(lane_permit);
                                            });
                                            dispatched += 1;
                                        }
                                        Err(error) => {
                                            // Gateway rejected the permit; put item back for retry
                                            self.reject_effect_item(&item, &error, true).await;
                                            drop(order_guard);
                                            drop(lane_permit);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if dispatched > 0 {
                tokio::select! {
                    _ = sleep(Duration::from_millis(200)) => {},
                    _ = self.shutdown.cancelled() => break,
                }
            } else {
                tokio::select! {
                    _ = self.coordination.wake.notified() => {},
                    _ = sleep(Duration::from_secs(30)) => {},
                    _ = self.shutdown.cancelled() => break,
                }
            }
        }
    }

    /// Mark an effect as succeeded without dispatching to the gateway.
    async fn skip_effect_item(&self, item: &EffectOutboxItem, reason: &str) {
        let receipt = serde_json::json!({"skipped": true, "reason": reason}).to_string();
        let _ = self
            .database
            .finish_effect_outbox(item.id, true, String::new(), receipt)
            .await;
    }

    /// Mark an effect as failed (or retryable) due to a middleware rejection.
    async fn reject_effect_item(&self, item: &EffectOutboxItem, error: &AppError, retryable: bool) {
        let msg = if retryable {
            format!("[middleware-retry] {}", error.message)
        } else {
            format!("[middleware-fail] {}", error.message)
        };
        let _ = self
            .database
            .finish_effect_outbox(item.id, false, msg, String::new())
            .await;
    }

    async fn dispatch_effect_permitted(
        &self,
        app: Option<&AppHandle>,
        item: EffectOutboxItem,
        _permit: &AutomaticWritePermit,
    ) {
        let work_id = runtime_lane_id(&item.effect_type, &item.account_id, item.group_id);
        let payload: Value = serde_json::from_str(&item.payload_json).unwrap_or(Value::Null);
        let user_id = payload.get("userId").and_then(Value::as_i64).unwrap_or(0);
        let duration = payload
            .get("durationSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let mut capabilities = self.gateway.capabilities();
        // The runtime starts its workers concurrently. A real inbound message
        // can therefore reach the effect queue before the background capability
        // worker has completed its first structural probe. Probe once here,
        // through the gateway's single-flight gate, rather than permanently
        // discarding a valid automatic effect as "not calibrated".
        if capability_for_effect(&capabilities, &item.effect_type).is_some_and(|value| {
            value.status == CapabilityStatus::ManualVerification && !value.automatic_allowed
        }) {
            capabilities = self.gateway.probe_capabilities("", "").await;
        }
        let capability = capability_for_effect(&capabilities, &item.effect_type);
        let result = if capability
            .as_ref()
            .is_some_and(|value| !value.automatic_allowed)
        {
            let retryable_probe = capability
                .as_ref()
                .is_some_and(|value| value.status == CapabilityStatus::ManualVerification);
            let error = AppError::new(
                "capability_automatic_disabled",
                if retryable_probe {
                    "协议能力仍在启动探测中，自动操作将按退避策略重试"
                } else {
                    "当前协议能力尚未开放自动执行，请查看调试页的探测说明"
                },
            );
            Err(if retryable_probe {
                error.retryable()
            } else {
                error
            })
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
                "rename" => {
                    let nickname = crate::cardnames::validate_card_name(
                        payload
                            .get("nickname")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                    match nickname {
                        Err(error) => Err(error),
                        Ok(nickname) => match self
                            .database
                            .expect_member_card_update(
                                item.account_id.clone(),
                                item.group_id,
                                user_id,
                                nickname.clone(),
                                item.dedupe_key.clone(),
                            )
                            .await
                        {
                            Err(error) => Err(error),
                            Ok(()) => {
                                let result = self
                                    .gateway
                                    .rename(
                                        item.group_id,
                                        &MemberRef {
                                            user_id: (user_id > 0).then_some(user_id),
                                            nim_id: payload
                                                .get("nimId")
                                                .and_then(Value::as_str)
                                                .map(str::to_string)
                                                .filter(|value| !value.trim().is_empty()),
                                        },
                                        &nickname,
                                    )
                                    .await;
                                let rename_failed = result
                                    .as_ref()
                                    .map(|receipt| receipt.status == "failed")
                                    .unwrap_or(true);
                                if rename_failed {
                                    let _ = self
                                        .database
                                        .consume_expected_member_card_update(
                                            item.account_id.clone(),
                                            item.group_id,
                                            user_id,
                                            nickname,
                                        )
                                        .await;
                                }
                                result
                            }
                        },
                    }
                }
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
        let (status, error_text, receipt) = match result {
            Ok(receipt) if receipt.status == "unknown" => (
                EffectDispatchStatus::Unknown,
                "外部操作已返回，但写后回读无法确认最终状态".into(),
                receipt,
            ),
            Ok(receipt) => (EffectDispatchStatus::Succeeded, String::new(), receipt),
            Err(error) => {
                let status = classify_effect_error(&error);
                let mut receipt = GatewayReceipt::failed(&item.effect_type, &error);
                if status == EffectDispatchStatus::Unknown {
                    receipt.status = "unknown".into();
                }
                (status, error.message.clone(), receipt)
            }
        };
        let success = status.succeeded();
        let error_text = redact(&error_text);
        let receipt = redact_gateway_receipt(receipt);
        let receipt_message_id = receipt.message_id.clone();
        let receipt_json = serde_json::to_string(&receipt).unwrap_or_default();
        let should_record_action = payload
            .get("recordAction")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let action_kind = payload
            .get("actionKind")
            .and_then(Value::as_str)
            .unwrap_or(&item.effect_type)
            .to_string();
        let action = should_record_action.then(|| ActionRecord {
            id: 0,
            account_id: item.account_id.clone(),
            group_id: item.group_id,
            user_id,
            message_id: payload.get("messageId").and_then(Value::as_i64),
            rule_id: payload.get("ruleId").and_then(Value::as_i64),
            kind: action_kind.clone(),
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
        });
        let status_name = match status {
            EffectDispatchStatus::Succeeded => "succeeded",
            EffectDispatchStatus::Failed => "failed",
            EffectDispatchStatus::TerminalFailed => "failed-terminal",
            EffectDispatchStatus::Unknown => "unknown",
        };
        let audit = AuditEvent {
            id: 0,
            account_id: item.account_id.clone(),
            group_id: item.group_id,
            user_id,
            actor: "DH BOT".into(),
            event: "effect_dispatched".into(),
            level: match status {
                EffectDispatchStatus::Succeeded => "info",
                EffectDispatchStatus::Failed | EffectDispatchStatus::TerminalFailed => "error",
                EffectDispatchStatus::Unknown => "warning",
            }
            .into(),
            details: serde_json::json!({
                "effect": item.effect_type,
                "actionKind": action_kind,
                "messageId": payload.get("messageId").and_then(Value::as_i64),
                "ruleId": payload.get("ruleId").and_then(Value::as_i64),
                "mode": payload.get("mode").and_then(Value::as_str).unwrap_or("automatic"),
                "reason": payload.get("reason").and_then(Value::as_str).unwrap_or_default(),
                "success": success,
                "status": status_name,
                "error": error_text,
                "receipt": receipt,
            })
            .to_string(),
            created_at: Utc::now(),
        };
        match self
            .database
            .archive_effect_dispatch(
                item.id,
                status_name.into(),
                error_text.clone(),
                receipt_json,
                action.clone(),
                audit,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                self.coordination
                    .tracker
                    .finish(&work_id, "failed", "操作结果没有完成归档");
                return;
            }
            Err(error) => {
                self.logger.write(
                    "ERROR",
                    &format!("副作用结果原子归档失败：{}", error.message),
                );
                self.coordination
                    .tracker
                    .finish(&work_id, "unknown", &error.message);
                return;
            }
        }

        if let Some(task_id) = payload.get("taskId").and_then(Value::as_i64) {
            if status == EffectDispatchStatus::Unknown {
                let _ = self
                    .database
                    .mark_task_reminder_unknown(task_id, error_text.clone())
                    .await;
            } else if success {
                let _ = self
                    .database
                    .finish_task_reminder(task_id, true, String::new())
                    .await;
            } else if status == EffectDispatchStatus::TerminalFailed || item.attempts >= 5 {
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
            if status == EffectDispatchStatus::Unknown {
                let _ = self
                    .database
                    .mark_schedule_run_unknown(run_key.to_string(), error_text.clone())
                    .await;
            } else if success
                || status == EffectDispatchStatus::TerminalFailed
                || item.attempts >= 5
            {
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
        if let Some(run_id) = payload.get("activityRunId").and_then(Value::as_i64) {
            let final_state = if status == EffectDispatchStatus::Unknown {
                Some("unknown")
            } else if success {
                Some("succeeded")
            } else if status == EffectDispatchStatus::TerminalFailed || item.attempts >= 5 {
                Some("failed")
            } else {
                None
            };
            if let Some(final_state) = final_state {
                let _ = self
                    .database
                    .finish_activity_run(run_id, final_state.into(), error_text.clone())
                    .await;
                let _ = self
                    .database
                    .record_audit(AuditEvent {
                        id: 0,
                        account_id: item.account_id.clone(),
                        group_id: item.group_id,
                        user_id: 0,
                        actor: "DH BOT".into(),
                        event: if success {
                            "activity_published"
                        } else {
                            "activity_publish_failed"
                        }
                        .into(),
                        level: if success {
                            "info"
                        } else if final_state == "unknown" {
                            "warning"
                        } else {
                            "error"
                        }
                        .into(),
                        details: serde_json::json!({
                            "activityId": payload.get("activityId").and_then(Value::as_i64),
                            "runId": run_id,
                            "state": final_state,
                            "contentSource": payload.get("contentSource").and_then(Value::as_str).unwrap_or("fixed"),
                            "messageId": receipt_message_id.clone(),
                            "error": error_text,
                        })
                        .to_string(),
                        created_at: Utc::now(),
                    })
                    .await;
            }
            if success {
                let now = self.clock.now_utc();
                let sender_id = self
                    .gateway
                    .session_identity()
                    .await
                    .map(|value| value.0)
                    .unwrap_or(0);
                let server_message_id = if receipt_message_id.trim().is_empty() {
                    format!("activity-run-{run_id}")
                } else {
                    receipt_message_id.clone()
                };
                if let Err(error) = self
                    .database
                    .insert_message(Message {
                        id: 0,
                        account_id: item.account_id.clone(),
                        group_id: item.group_id,
                        server_message_id,
                        sequence: 0,
                        user_id: sender_id,
                        sender_name: "DH BOT".into(),
                        kind: "text".into(),
                        text: payload
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        sent_at: now,
                        received_at: now,
                        processed_at: Some(now),
                        acknowledged_at: Some(now),
                        processing_state: "processed".into(),
                        attempts: 1,
                        next_attempt_at: None,
                        last_error: String::new(),
                        mentions_json: "[]".into(),
                        source_kind: Some("activity".into()),
                        flow: Some("outgoing".into()),
                    })
                    .await
                {
                    self.logger.write(
                        "ERROR",
                        &format!("活动消息写入 AI 会话失败：{}", error.message),
                    );
                }
            }
            if let Some(app) = app {
                let _ = app.emit(
                    "activity-updated",
                    serde_json::json!({"runId":run_id,"success":success,"state":final_state,"error":error_text}),
                );
            }
        }
        if success {
            if let Some(card_job_id) = payload.get("cardJobId").and_then(Value::as_i64) {
                let _ = self.database.mark_card_welcome_sent(card_job_id).await;
            }
        }
        if action.is_some() {
            if let Some(app) = app {
                let _ = app.emit(
                    "action-recorded",
                    json_action(&action_kind, success, &error_text),
                );
            }
        }
        match status {
            EffectDispatchStatus::Succeeded => {
                self.coordination.tracker.finish(&work_id, "succeeded", "")
            }
            EffectDispatchStatus::Failed if item.attempts < 5 => self
                .coordination
                .tracker
                .retrying(&work_id, None, &error_text),
            EffectDispatchStatus::Unknown => {
                self.coordination
                    .tracker
                    .finish(&work_id, "unknown", &error_text)
            }
            EffectDispatchStatus::Failed | EffectDispatchStatus::TerminalFailed => self
                .coordination
                .tracker
                .finish(&work_id, "failed", &error_text),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_incoming_job(
        &self,
        sender: &mpsc::Sender<IncomingJob>,
        account_id: &str,
        job: IncomingJob,
    ) {
        let work_id = runtime_lane_id("message", account_id, job.message.group_id);
        self.coordination
            .tracker
            .enqueue(&work_id, "message", "处理群消息", "群消息队列");
        if let Err(error) = sender.try_send(job) {
            let reason = match error {
                mpsc::error::TrySendError::Full(_) => "群消息队列繁忙，已保留到持久队列等待重试",
                mpsc::error::TrySendError::Closed(_) => "群消息处理队列已关闭",
            };
            self.coordination.tracker.reject_enqueue(&work_id, reason);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn worker_for(
        &self,
        workers: &mut HashMap<(String, u64, i64), mpsc::Sender<IncomingJob>>,
        worker_tasks: &mut JoinSet<()>,
        group_id: i64,
        session_epoch: u64,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
    ) -> mpsc::Sender<IncomingJob> {
        workers
            .entry((account_id.to_string(), session_epoch, group_id))
            .or_insert_with(|| {
                let (sender, mut receiver) = mpsc::channel::<IncomingJob>(128);
                let (ai_sender, mut ai_receiver) = mpsc::channel::<AiReplyJob>(128);
                let (ai_rule_sender, mut ai_rule_receiver) = mpsc::channel::<AiRuleJob>(128);
                let rule_runtime = self.clone();
                let rule_app = app.clone();
                worker_tasks.spawn(async move {
                    while let Some(job) = ai_rule_receiver.recv().await {
                        let work_id =
                            runtime_lane_id("aiRule", &job.account_id, job.group.group_id);
                        let Ok(_permit) = rule_runtime.ai_rule_gate.clone().acquire_owned().await
                        else {
                            rule_runtime.coordination.tracker.start(&work_id, None);
                            rule_runtime.coordination.tracker.finish(
                                &work_id,
                                "failed",
                                "AI 调度器已停止",
                            );
                            break;
                        };
                        rule_runtime
                            .process_ai_rules_background(
                                rule_app.clone(),
                                job.account_id,
                                job.sender_id,
                                job.group,
                                job.member,
                                job.message,
                                job.recent_events,
                                job.rules,
                            )
                            .await;
                    }
                });
                let ai_runtime = self.clone();
                let ai_app = app.clone();
                let ai_account = account_id.to_string();
                worker_tasks.spawn(async move {
                    while let Some(job) = ai_receiver.recv().await {
                        let work_id = runtime_lane_id("aiReply", &ai_account, job.message.group_id);
                        let Ok(_permit) = ai_runtime.ai_rule_gate.clone().acquire_owned().await
                        else {
                            ai_runtime.coordination.tracker.start(&work_id, None);
                            ai_runtime.coordination.tracker.finish(
                                &work_id,
                                "failed",
                                "AI 调度器已停止",
                            );
                            break;
                        };
                        ai_runtime.coordination.tracker.start(&work_id, None);
                        match ai_runtime
                            .process_deferred_ai(
                                &ai_app,
                                &ai_account,
                                sender_id,
                                job.message.clone(),
                            )
                            .await
                        {
                            Ok(()) => {
                                ai_runtime
                                    .coordination
                                    .tracker
                                    .finish(&work_id, "succeeded", "")
                            }
                            Err(error) => {
                                ai_runtime.coordination.tracker.finish(
                                    &work_id,
                                    "failed",
                                    &error.message,
                                );
                                let _ = ai_runtime
                                    .database
                                    .record_audit(AuditEvent {
                                        id: 0,
                                        account_id: ai_account.clone(),
                                        group_id: job.message.group_id,
                                        user_id: job.message.user_id,
                                        actor: "DH BOT".into(),
                                        event: "ai_reply_worker_failed".into(),
                                        level: "warning".into(),
                                        details: serde_json::json!({
                                            "messageId": job.message.id,
                                            "status": "failed",
                                            "error": error.message,
                                        })
                                        .to_string(),
                                        created_at: Utc::now(),
                                    })
                                    .await;
                            }
                        }
                    }
                });
                let runtime = self.clone();
                let app_handle = app.clone();
                let account = account_id.to_string();
                worker_tasks.spawn(async move {
                    while let Some(job) = receiver.recv().await {
                        let work_id = runtime_lane_id("message", &account, job.message.group_id);
                        runtime.coordination.tracker.start(&work_id, Some(4));
                        if runtime
                            .database
                            .claim_message(job.message.id)
                            .await
                            .unwrap_or(false)
                        {
                            runtime.coordination.tracker.progress(&work_id, 1, 4);
                            let result = runtime
                                .process_job(
                                    &app_handle,
                                    &account,
                                    sender_id,
                                    job.clone(),
                                    &ai_sender,
                                    &ai_rule_sender,
                                    &work_id,
                                )
                                .await;
                            let (success, error) = match &result {
                                Ok(()) => (true, String::new()),
                                Err(error) => (false, error.message.clone()),
                            };
                            runtime.coordination.tracker.finish(
                                &work_id,
                                if success { "succeeded" } else { "failed" },
                                &error,
                            );
                            let _ = runtime
                                .database
                                .finish_message_processing(job.message.id, success, error)
                                .await;
                        } else {
                            runtime
                                .coordination
                                .tracker
                                .finish(&work_id, "succeeded", "");
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

    async fn activity_loop(&self, app: AppHandle) {
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

    async fn is_group_manager(&self, sender_id: i64, group_id: i64) -> bool {
        self.gateway
            .list_members(group_id)
            .await
            .ok()
            .and_then(|roster| {
                roster.members.into_iter().find(|member| {
                    member.user_id == sender_id && member.user_id > 0 && member.present
                })
            })
            .map(|member| matches!(member.role.as_str(), "owner" | "admin"))
            .unwrap_or(false)
    }

    async fn roster_sync_loop(&self) {
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

    async fn sync_members(&self, account_id: &str) {
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

    async fn enqueue_discovered_member_card_jobs(
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

    async fn schedule_loop(&self, app: AppHandle) {
        loop {
            if self.gateway.diagnose().await.status == ConnectionStatus::Ready {
                if let Ok((sender_id, account_id)) = self.gateway.session_identity().await {
                    if let Ok(schedules) = self.database.list_schedules(account_id.clone()).await {
                        for schedule in schedules.into_iter().filter(|schedule| schedule.enabled) {
                            for group_id in &schedule.group_ids {
                                let Ok(decision) = scheduler::evaluate(
                                    &schedule,
                                    *group_id,
                                    self.clock.now_local(),
                                ) else {
                                    continue;
                                };
                                let Ok(_permit) = self.gateway.automatic_write_permit().await
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
                                let work_id = runtime_lane_id("schedule", &account_id, *group_id);
                                self.coordination.tracker.start_named(
                                    &work_id,
                                    "schedule",
                                    "执行群计划",
                                    "计划任务",
                                    None,
                                );
                                let allowed = self
                                    .gateway
                                    .list_members(*group_id)
                                    .await
                                    .ok()
                                    .and_then(|roster| {
                                        roster.members.into_iter().find(|member| {
                                            member.user_id == sender_id
                                                && member.user_id > 0
                                                && member.present
                                        })
                                    })
                                    .map(|member| matches!(member.role.as_str(), "owner" | "admin"))
                                    .unwrap_or(false);
                                let result = if allowed
                                    && self.gateway.capabilities().group_mute.automatic_allowed
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
                                self.coordination.tracker.finish(
                                    &work_id,
                                    if success { "succeeded" } else { "failed" },
                                    &error,
                                );
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
            tokio::select! { _ = self.coordination.wake.notified() => {}, _ = sleep(Duration::from_secs(30)) => {}, _ = self.shutdown.cancelled() => break }
        }
    }

    async fn connection_loop(&self, app: AppHandle) {
        let mut workers: HashMap<(String, u64, i64), mpsc::Sender<IncomingJob>> = HashMap::new();
        let mut worker_tasks = JoinSet::new();
        let mut member_event_cache: HashMap<String, Vec<GatewayEvent>> = HashMap::new();
        let mut reported_member_event_mismatches = HashSet::new();
        let mut reported_ignored_events = HashSet::new();
        let mut active_account = String::new();
        let mut session_epoch = 0u64;
        let mut last_retry_scan = Utc::now() - chrono::Duration::seconds(5);
        let mut connection_retry = Duration::from_millis(250);
        'connection: loop {
            let diagnostic = self.gateway.diagnose().await;
            self.events.emit(
                "connection-status",
                serde_json::to_value(&diagnostic).unwrap_or(Value::Null),
            );
            let _ = app.emit("gateway-capabilities", self.gateway.capabilities());
            if diagnostic.status != ConnectionStatus::Ready {
                let delay = connection_retry;
                connection_retry = next_connection_retry(connection_retry);
                tokio::select! { _ = sleep(delay) => {}, _ = self.shutdown.cancelled() => break }
                continue;
            }
            if self.gateway.install_message_listener().await.is_err() {
                let delay = connection_retry;
                connection_retry = next_connection_retry(connection_retry);
                tokio::select! { _ = sleep(delay) => {}, _ = self.shutdown.cancelled() => break }
                continue;
            }
            let (sender_id, account_id) = match self.gateway.session_identity().await {
                Ok(identity) => identity,
                Err(error) => {
                    self.logger.write("WARN", &error.message);
                    let _ = app.emit("connection-error", &error);
                    let delay = connection_retry;
                    connection_retry = next_connection_retry(connection_retry);
                    tokio::select! { _ = sleep(delay) => {}, _ = self.shutdown.cancelled() => break };
                    continue;
                }
            };
            let session_started = Instant::now();
            let gateway_epoch = self.gateway.session_epoch();
            if active_account != account_id || session_epoch != gateway_epoch {
                workers.clear();
                worker_tasks.abort_all();
                member_event_cache.clear();
                reported_member_event_mismatches.clear();
                reported_ignored_events.clear();
                active_account = account_id.clone();
                session_epoch = gateway_epoch;
                last_retry_scan = Utc::now() - chrono::Duration::seconds(5);
            }
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
            let _ = self
                .database
                .ensure_account_defaults(account_id.clone())
                .await;
            let sync_work_id = runtime_lane_id("sync", &account_id, 0);
            self.coordination.tracker.start_named(
                &sync_work_id,
                "sync",
                "同步群资料",
                "旺商聊",
                Some(2),
            );
            match self.gateway.list_groups().await {
                Ok(groups) => {
                    self.coordination.tracker.progress(&sync_work_id, 1, 2);
                    for group in groups {
                        let _ = self.database.upsert_group(group).await;
                    }
                    self.coordination.tracker.progress(&sync_work_id, 2, 2);
                    self.coordination
                        .tracker
                        .finish(&sync_work_id, "succeeded", "");
                }
                Err(error) => {
                    self.coordination
                        .tracker
                        .finish(&sync_work_id, "failed", &error.message)
                }
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
                        &mut worker_tasks,
                        message.group_id,
                        session_epoch,
                        &app,
                        &account_id,
                        sender_id,
                    );
                    self.enqueue_incoming_job(
                        &sender,
                        &account_id,
                        IncomingJob {
                            sequence: message.sequence.max(0) as u64,
                            message,
                        },
                    );
                }
            }
            let mut last_batch_session: Option<String> = None;
            let mut last_ack_sequence: Option<u64> = None;
            'batch: loop {
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
                                &mut worker_tasks,
                                message.group_id,
                                session_epoch,
                                &app,
                                &account_id,
                                sender_id,
                            );
                            self.enqueue_incoming_job(
                                &sender,
                                &account_id,
                                IncomingJob {
                                    sequence: message.sequence.max(0) as u64,
                                    message,
                                },
                            );
                        }
                    }
                    if let Ok(inbox) = self
                        .database
                        .claim_gateway_inbox(Some(account_id.clone()), 1)
                        .await
                    {
                        for item in inbox {
                            if item.event_type != "message" {
                                if item.event_type == "connection-changed" {
                                    match self
                                        .gateway
                                        .ack(
                                            &item.bridge_session,
                                            item.bridge_sequence.max(0) as u64,
                                        )
                                        .await
                                    {
                                        Ok(_) => {
                                            if let Err(error) = self
                                                .database
                                                .commit_gateway_ack(
                                                    account_id.clone(),
                                                    vec![item.event_id.clone()],
                                                    Vec::new(),
                                                )
                                                .await
                                            {
                                                let _ = self
                                                    .database
                                                    .finish_gateway_inbox(
                                                        item.id,
                                                        false,
                                                        format!(
                                                            "源队列 ACK 已成功，本地提交失败：{}",
                                                            error.message
                                                        ),
                                                    )
                                                    .await;
                                            }
                                        }
                                        Err(error) => {
                                            let _ = self
                                                .database
                                                .finish_gateway_inbox(item.id, false, error.message)
                                                .await;
                                        }
                                    }
                                } else {
                                    let _ = self
                                        .database
                                        .finish_gateway_inbox(
                                            item.id,
                                            false,
                                            "成员事件等待源队列重放和名单同步确认".into(),
                                        )
                                        .await;
                                }
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
                                object.insert(
                                    "listenerSession".into(),
                                    Value::from(item.bridge_session.clone()),
                                );
                            }
                            match self.normalize_message(&account_id, sender_id, &raw).await {
                                Ok(mut job) => {
                                    match self.database.insert_message(job.message.clone()).await {
                                        Ok(persisted) => {
                                            job.message.id = persisted.id;
                                            match self
                                                .gateway
                                                .ack(
                                                    &item.bridge_session,
                                                    item.bridge_sequence.max(0) as u64,
                                                )
                                                .await
                                            {
                                                Ok(_) => match self
                                                    .database
                                                    .commit_gateway_ack(
                                                        account_id.clone(),
                                                        vec![item.event_id.clone()],
                                                        vec![persisted.id],
                                                    )
                                                    .await
                                                {
                                                    Ok(()) => {
                                                        if !persisted.processed {
                                                            let sender = self.worker_for(
                                                                &mut workers,
                                                                &mut worker_tasks,
                                                                job.message.group_id,
                                                                session_epoch,
                                                                &app,
                                                                &account_id,
                                                                sender_id,
                                                            );
                                                            self.enqueue_incoming_job(
                                                                &sender,
                                                                &account_id,
                                                                job,
                                                            );
                                                        }
                                                    }
                                                    Err(error) => {
                                                        let _ = self
                                                            .database
                                                            .finish_gateway_inbox(
                                                                item.id,
                                                                false,
                                                                format!(
                                                                    "源队列 ACK 已成功，本地提交失败：{}",
                                                                    error.message
                                                                ),
                                                            )
                                                            .await;
                                                    }
                                                },
                                                Err(error) => {
                                                    let _ = self
                                                        .database
                                                        .finish_gateway_inbox(
                                                            item.id,
                                                            false,
                                                            error.message,
                                                        )
                                                        .await;
                                                }
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
                                    let retry_error =
                                        format!("{reason}；已保留事件，等待群列表映射恢复后重试");
                                    if let Err(error) = self
                                        .database
                                        .finish_gateway_inbox(item.id, false, retry_error)
                                        .await
                                    {
                                        self.logger.write("WARN", &error.message);
                                    }
                                }
                                Err(reason) => {
                                    match self
                                        .gateway
                                        .ack(
                                            &item.bridge_session,
                                            item.bridge_sequence.max(0) as u64,
                                        )
                                        .await
                                    {
                                        Ok(_) => {
                                            if let Err(error) = self
                                                .database
                                                .commit_gateway_ack(
                                                    account_id.clone(),
                                                    vec![item.event_id.clone()],
                                                    Vec::new(),
                                                )
                                                .await
                                            {
                                                let _ = self
                                                    .database
                                                    .finish_gateway_inbox(
                                                        item.id,
                                                        false,
                                                        format!(
                                                            "源队列 ACK 已成功，本地提交失败：{}",
                                                            error.message
                                                        ),
                                                    )
                                                    .await;
                                            }
                                        }
                                        Err(error) => {
                                            let _ = self
                                                .database
                                                .finish_gateway_inbox(
                                                    item.id,
                                                    false,
                                                    format!("{reason}；{}", error.message),
                                                )
                                                .await;
                                        }
                                    }
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
                if let Some(previous_session) = &last_batch_session {
                    if previous_session != &batch.session {
                        let _ = app.emit(
                            "gateway-resync-required",
                            serde_json::json!({
                                "session": batch.session.clone(),
                                "reason": "listener_session_changed"
                            }),
                        );
                        break 'batch;
                    }
                } else {
                    last_batch_session = Some(batch.session.clone());
                }
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
                    let _ = app.emit(
                        "gateway-resync-required",
                        serde_json::json!({
                            "session": batch.session.clone(),
                            "dropped": batch.dropped,
                            "reason": "source_queue_overflow"
                        }),
                    );
                    break 'batch;
                }
                if batch
                    .records
                    .windows(2)
                    .any(|pair| pair[1].sequence != pair[0].sequence.saturating_add(1))
                {
                    let _ = app.emit(
                        "gateway-resync-required",
                        serde_json::json!({
                            "session": batch.session.clone(),
                            "reason": "sequence_gap"
                        }),
                    );
                    break 'batch;
                }
                if let (Some(last_ack), Some(first_record)) =
                    (last_ack_sequence, batch.records.first())
                {
                    if first_record.sequence > last_ack.saturating_add(1) {
                        let _ = app.emit(
                            "gateway-resync-required",
                            serde_json::json!({
                                "session": batch.session.clone(),
                                "lastAcknowledged": last_ack,
                                "firstAvailable": first_record.sequence,
                                "reason": "sequence_gap"
                            }),
                        );
                        break 'batch;
                    }
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
                let mut ackable_event_ids = HashSet::new();
                let mut member_event_ids = Vec::new();
                let mut expected_member_events = 0usize;
                for record in &batch.records {
                    let inbox_event_id = gateway_record_id(record);
                    match record.kind {
                        GatewayRecordKind::ConnectionChanged => {
                            ackable_event_ids.insert(inbox_event_id);
                            continue;
                        }
                        GatewayRecordKind::TeamMemberJoined
                        | GatewayRecordKind::TeamMemberLeft
                        | GatewayRecordKind::TeamMemberUpdated => {
                            expected_member_events +=
                                ["members", "teamMembers", "accounts", "memberList"]
                                    .iter()
                                    .find_map(|key| {
                                        record.payload.get(*key).and_then(Value::as_array)
                                    })
                                    .map(Vec::len)
                                    .or_else(|| {
                                        record
                                            .payload
                                            .get("member")
                                            .or_else(|| record.payload.get("account"))
                                            .filter(|value| value.is_object())
                                            .map(|_| 1)
                                    })
                                    .unwrap_or(0);
                            member_event_ids.push(inbox_event_id);
                            continue;
                        }
                        GatewayRecordKind::Message => {}
                    }
                    let mut raw = record.payload.clone();
                    if let Some(object) = raw.as_object_mut() {
                        object.insert("seq".into(), Value::from(record.sequence));
                        object.insert("source".into(), Value::from(record.source.clone()));
                        object.insert(
                            "listenerSession".into(),
                            Value::from(record.session.clone()),
                        );
                    }
                    match self.normalize_message(&account_id, sender_id, &raw).await {
                        Ok(job) => {
                            ackable_event_ids.insert(inbox_event_id);
                            normalized.push((job, raw));
                        }
                        Err(reason) => {
                            if reason != "解码失败且群身份未映射" {
                                ackable_event_ids.insert(inbox_event_id.clone());
                            }
                            ignored.push((
                                inbox_event_id,
                                record.sequence,
                                record.source.clone(),
                                reason,
                            ));
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
                        tokio::select! { _ = sleep(Duration::from_millis(700)) => {}, _ = self.shutdown.cancelled() => break 'connection }
                        continue;
                    }
                };
                // A temporarily missing team-to-group mapping is not malformed
                // input. Keep the durable inbox row pending and withhold the
                // source ACK so a successful group-list refresh can replay it.
                // Member records join the ACK prefix only after every normalized
                // event has been durably applied. Transient database failures are
                // retried in-place so a gateway cursor cannot outrun persistence.
                let member_cache_key = (!member_event_ids.is_empty())
                    .then(|| format!("{}:{}", batch.session, member_event_ids.join("|")));
                if !member_event_ids.is_empty() {
                    let already_processed = self
                        .database
                        .processed_gateway_event_ids(account_id.clone(), member_event_ids.clone())
                        .await
                        .unwrap_or_default();
                    if already_processed.len() == member_event_ids.len() {
                        ackable_event_ids.extend(already_processed);
                    } else {
                        let member_records = batch
                            .records
                            .iter()
                            .filter(|record| {
                                matches!(
                                    record.kind,
                                    GatewayRecordKind::TeamMemberJoined
                                        | GatewayRecordKind::TeamMemberLeft
                                        | GatewayRecordKind::TeamMemberUpdated
                                )
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let events = if let Some(cached) = member_cache_key
                            .as_ref()
                            .and_then(|key| member_event_cache.get(key))
                        {
                            Ok(cached.clone())
                        } else {
                            self.gateway.member_events(member_records).await
                        };
                        match events {
                            Ok(events) if events.len() == expected_member_events => {
                                if let Some(key) = &member_cache_key {
                                    // Evict one arbitrary entry when the cache is full so it
                                    // cannot grow without bound across a long-lived connection.
                                    if member_event_cache.len() >= MAX_MEMBER_EVENT_CACHE
                                        && !member_event_cache.contains_key(key.as_str())
                                    {
                                        if let Some(oldest) =
                                            member_event_cache.keys().next().cloned()
                                        {
                                            member_event_cache.remove(&oldest);
                                        }
                                    }
                                    member_event_cache.insert(key.clone(), events.clone());
                                }
                                for (event_index, event) in events.into_iter().enumerate() {
                                    let durable_event_id = format!(
                                        "{}:{event_index}",
                                        member_cache_key.as_deref().unwrap_or(&batch.session)
                                    );
                                    loop {
                                        match self
                                            .handle_gateway_event(
                                                &app,
                                                &account_id,
                                                &durable_event_id,
                                                event.clone(),
                                            )
                                            .await
                                        {
                                            Ok(()) => break,
                                            Err(error) => {
                                                self.logger.write(
                                                    "WARN",
                                                    &format!(
                                                        "成员事件持久化失败，稍后重试：{}",
                                                        error.message
                                                    ),
                                                );
                                                let _ = app.emit("connection-error", &error);
                                                tokio::select! {
                                                    _ = sleep(Duration::from_millis(500)) => {},
                                                    _ = self.shutdown.cancelled() => break 'connection,
                                                }
                                            }
                                        }
                                    }
                                }
                                loop {
                                    match self
                                        .database
                                        .mark_gateway_inbox_processed(
                                            account_id.clone(),
                                            member_event_ids.clone(),
                                        )
                                        .await
                                    {
                                        Ok(_) => break,
                                        Err(error) => {
                                            self.logger.write(
                                                "WARN",
                                                &format!(
                                                    "成员事件状态写入失败，稍后重试：{}",
                                                    error.message
                                                ),
                                            );
                                            tokio::select! {
                                                _ = sleep(Duration::from_millis(500)) => {},
                                                _ = self.shutdown.cancelled() => break 'connection,
                                            }
                                        }
                                    }
                                }
                                ackable_event_ids.extend(member_event_ids.iter().cloned());
                            }
                            Ok(events) => {
                                let actual = events.len();
                                let mismatch_key = member_cache_key
                                    .clone()
                                    .unwrap_or_else(|| member_event_ids.join("|"));
                                if reported_member_event_mismatches.len() < MAX_REPORTED_SET
                                    && reported_member_event_mismatches.insert(mismatch_key) {
                                    self.logger.write(
                                        "WARN",
                                        &format!(
                                            "成员事件未完整识别：预期 {expected_member_events}，实际 {actual}；已转为 60 秒名单对账"
                                        ),
                                    );
                                    let _ = self
                                        .database
                                        .record_audit(AuditEvent {
                                            id: 0,
                                            account_id: account_id.clone(),
                                            group_id: 0,
                                            user_id: 0,
                                            actor: "DH BOT".into(),
                                            event: "member_event_fallback".into(),
                                            level: "warning".into(),
                                            details: serde_json::json!({
                                                "expected": expected_member_events,
                                                "normalized": actual,
                                                "result": "ignored",
                                                "fallback": "roster-reconciliation",
                                                "reason": "成员事件缺少可映射的群或成员身份"
                                            })
                                            .to_string(),
                                            created_at: Utc::now(),
                                        })
                                        .await;
                                }
                                match self
                                    .database
                                    .mark_gateway_inbox_processed(
                                        account_id.clone(),
                                        member_event_ids.clone(),
                                    )
                                    .await
                                {
                                    Ok(_) => {
                                        ackable_event_ids.extend(member_event_ids.iter().cloned())
                                    }
                                    Err(error) => self.logger.write(
                                        "WARN",
                                        &format!(
                                            "成员事件转入名单对账时写入状态失败：{}",
                                            error.message
                                        ),
                                    ),
                                }
                            }
                            Err(error) => {
                                self.logger.write("WARN", &error.message);
                                let _ = app.emit("connection-error", &error);
                            }
                        }
                    }
                }
                for (event_id, sequence, source, reason) in ignored {
                    if reported_ignored_events.len() >= MAX_REPORTED_SET
                        || !reported_ignored_events.insert(event_id)
                    {
                        continue;
                    }
                    let mapping_pending = reason == "解码失败且群身份未映射";
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.clone(),
                            group_id: 0,
                            user_id: 0,
                            actor: "DH BOT".into(),
                            event: if mapping_pending {
                                "message_mapping_pending"
                            } else {
                                "message_ignored"
                            }
                            .into(),
                            level: if mapping_pending {
                                "warning"
                            } else {
                                "info"
                            }
                            .into(),
                            details: serde_json::json!({
                                "direction": "incoming",
                                "sequence": sequence,
                                "source": source,
                                "processingState": if mapping_pending { "retry" } else { "ignored" },
                                "reason": reason,
                            })
                            .to_string(),
                            created_at: Utc::now(),
                        })
                        .await;
                }
                let ack_sequence = contiguous_ack_sequence(&batch.records, &ackable_event_ids);
                if ack_sequence > 0 {
                    if let Err(error) = self.gateway.ack(&batch.session, ack_sequence).await {
                        self.logger.write("WARN", &error.message);
                    } else {
                        last_ack_sequence = Some(ack_sequence);
                        let completed_inbox_ids = batch
                            .records
                            .iter()
                            .take_while(|record| record.sequence <= ack_sequence)
                            .map(gateway_record_id)
                            .collect::<Vec<_>>();
                        let acknowledged_message_ids = normalized
                            .iter()
                            .zip(&persisted_messages)
                            .filter(|((job, _), _)| job.sequence <= ack_sequence)
                            .map(|(_, persisted)| persisted.id)
                            .collect::<Vec<_>>();
                        let local_commit = self
                            .database
                            .commit_gateway_ack(
                                account_id.clone(),
                                completed_inbox_ids,
                                acknowledged_message_ids,
                            )
                            .await;
                        if let Err(error) = local_commit {
                            self.logger.write(
                                "WARN",
                                &format!(
                                    "源队列 ACK 已成功，本地状态提交失败，将重试：{}",
                                    error.message
                                ),
                            );
                        } else {
                            if member_event_ids
                                .iter()
                                .all(|event_id| ackable_event_ids.contains(event_id))
                                && batch
                                    .records
                                    .iter()
                                    .filter(|record| {
                                        member_event_ids.contains(&gateway_record_id(record))
                                    })
                                    .all(|record| record.sequence <= ack_sequence)
                            {
                                if let Some(key) = &member_cache_key {
                                    member_event_cache.remove(key);
                                }
                            }
                            for ((mut job, raw), persisted) in
                                normalized.into_iter().zip(persisted_messages)
                            {
                                if persisted.processed || job.sequence > ack_sequence {
                                    continue;
                                }
                                job.message.id = persisted.id;
                                if persisted.inserted {
                                    let _ = self
                                        .database
                                        .record_audit(AuditEvent {
                                            id: 0,
                                            account_id: account_id.clone(),
                                            group_id: job.message.group_id,
                                            user_id: job.message.user_id,
                                            actor: "DH BOT".into(),
                                            event: "message_received".into(),
                                            level: "info".into(),
                                            details: message_audit_details(
                                                &job.message,
                                                job.sequence,
                                                "queued",
                                                "persisted-and-acknowledged",
                                            ),
                                            created_at: Utc::now(),
                                        })
                                        .await;
                                }
                                let sender = self.worker_for(
                                    &mut workers,
                                    &mut worker_tasks,
                                    job.message.group_id,
                                    session_epoch,
                                    &app,
                                    &account_id,
                                    sender_id,
                                );
                                self.enqueue_incoming_job(&sender, &account_id, job);
                                let _ = app.emit("message-received", &raw);
                            }
                        }
                    }
                }
                tokio::select! { _ = sleep(Duration::from_millis(700)) => {}, _ = self.shutdown.cancelled() => break 'connection }
            }
            member_event_cache.clear();
            reported_member_event_mismatches.clear();
            reported_ignored_events.clear();
            last_retry_scan = Utc::now() - chrono::Duration::seconds(5);
            // 只有真正稳定跑过一段时间的会话才清空退避，否则反复 break 的会话
            // （队列溢出、序列号缺口）会一直以最短间隔重连，把网关打穿。
            if session_started.elapsed() >= HEALTHY_SESSION_THRESHOLD {
                connection_retry = Duration::from_millis(250);
            }
            let delay = connection_retry;
            connection_retry = next_connection_retry(connection_retry);
            tokio::select! { _ = sleep(delay) => {}, _ = self.shutdown.cancelled() => break }
        }
        drop(workers);
        while worker_tasks.join_next().await.is_some() {}
    }

    async fn handle_gateway_event(
        &self,
        app: &AppHandle,
        account_id: &str,
        durable_event_id: &str,
        event: GatewayEvent,
    ) -> AppResult<()> {
        match event {
            GatewayEvent::MemberJoined {
                group_id,
                mut member,
            } => {
                member.account_id = account_id.into();
                if let Some(saved) = self
                    .database
                    .list_members(account_id.to_string(), group_id)
                    .await?
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
                self.database.upsert_member(member.clone()).await?;
                let _ = app.emit("sync-progress", serde_json::json!({"phase":"member-joined","groupId":group_id,"userId":member.user_id,"source":"online-joined"}));
                if member.blacklisted {
                    self.enqueue_effect(
                        account_id,
                        group_id,
                        "remove",
                        serde_json::json!({
                            "userId": member.user_id,
                            "recordAction": true,
                            "actionKind": "remove",
                            "reason": "黑名单成员重新入群",
                            "mode": "automatic",
                        }),
                        format!("member-rejoin-remove:{account_id}:{durable_event_id}"),
                    )
                    .await?;
                    self.database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "blacklisted_member_rejoined".into(),
                            level: "warning".into(),
                            details: "黑名单成员重新入群，已进入移出队列".into(),
                            created_at: Utc::now(),
                        })
                        .await?;
                    return Ok(());
                }
                self.enqueue_member_card_job(account_id, group_id, &member)
                    .await?;
            }
            GatewayEvent::MemberLeft { group_id, member } => {
                self.database
                    .mark_member_not_present(account_id.to_string(), group_id, member.user_id)
                    .await?;
                self.database
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
                    .await?;
            }
            GatewayEvent::MemberUpdated {
                group_id,
                mut member,
            } => {
                member.account_id = account_id.into();
                let saved = self
                    .database
                    .list_members(account_id.to_string(), group_id)
                    .await?
                    .into_iter()
                    .find(|saved| {
                        saved.user_id == member.user_id
                            || (!member.nim_id.is_empty() && saved.nim_id == member.nim_id)
                    });
                let previous_card_name = saved
                    .as_ref()
                    .map(|saved| saved.card_name.trim().to_string())
                    .unwrap_or_default();
                let incoming_card_name = member.card_name.trim().to_string();
                let expected_update = if saved.is_some()
                    && !incoming_card_name.is_empty()
                    && previous_card_name != incoming_card_name
                {
                    self.database
                        .consume_expected_member_card_update(
                            account_id.to_string(),
                            group_id,
                            member.user_id,
                            incoming_card_name.clone(),
                        )
                        .await?
                } else {
                    false
                };
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
                let new_violation = is_external_member_card_update(
                    saved.is_some(),
                    restore_name.is_some(),
                    &previous_card_name,
                    &incoming_card_name,
                    expected_update,
                );
                self.database.upsert_member(member.clone()).await?;
                if new_violation {
                    member.violation_count = self
                        .database
                        .increment_member_violation(
                            account_id.to_string(),
                            group_id,
                            member.user_id,
                        )
                        .await?;
                    self.process_member_update_rules(
                        app,
                        account_id,
                        durable_event_id,
                        group_id,
                        &member,
                    )
                    .await;
                }
                if let Some(locked) = restore_name {
                    self.enqueue_effect(
                        account_id,
                        group_id,
                        "rename",
                        serde_json::json!({
                            "userId": member.user_id,
                            "nimId": member.nim_id,
                            "nickname": locked,
                            "recordAction": true,
                            "actionKind": "rename",
                            "reason": "锁定群名片恢复",
                            "mode": "automatic",
                        }),
                        format!("locked-card:{account_id}:{durable_event_id}"),
                    )
                    .await?;
                    self.database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "locked_card_restore_queued".into(),
                            level: "warning".into(),
                            details: serde_json::json!({
                                "lockedName": locked,
                                "violationCount": member.violation_count,
                                "action": "queued-for-restore",
                            })
                            .to_string(),
                            created_at: Utc::now(),
                        })
                        .await?;
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
        Ok(())
    }

    async fn process_member_update_rules(
        &self,
        app: &AppHandle,
        account_id: &str,
        durable_event_id: &str,
        group_id: i64,
        member: &Member,
    ) {
        let Some(_group) = self
            .database
            .list_groups(Some(account_id.to_string()))
            .await
            .ok()
            .and_then(|groups| groups.into_iter().find(|group| group.group_id == group_id))
            .filter(|group| group.enabled && group.machine_rules_enabled)
        else {
            return;
        };
        let rules = self
            .database
            .list_rules(account_id.to_string(), Some(group_id))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|rule| rule.enabled && !rule.is_ai_rule() && rule.matcher == "rename_count")
            .collect::<Vec<_>>();
        if rules.is_empty() {
            return;
        }
        let input = moderation::ModerationInput {
            member,
            kind: "member_updated",
            text: "",
            now: self.clock.now_utc(),
            recent: &[],
            rename_violations: member.violation_count,
        };
        let started = std::time::Instant::now();
        let decision =
            moderation::evaluate_with_classifier(&rules, &input, self.semantic_classifier.as_ref());
        let elapsed_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
        let event_message_id = durable_event_numeric_id(durable_event_id);
        for rule in &rules {
            let matched = decision
                .matches
                .iter()
                .find(|matched| matched.rule_id == rule.id);
            let _ = self
                .database
                .record_rule_evaluation(
                    account_id.into(),
                    group_id,
                    member.user_id,
                    event_message_id,
                    rule.id,
                    "machine".into(),
                    matched.is_some(),
                    None,
                    rule.mode.clone(),
                    if matched.is_some() {
                        "命中".into()
                    } else {
                        "未命中".into()
                    },
                    matched
                        .map(|value| value.reason.clone())
                        .unwrap_or_default(),
                    elapsed_ms,
                )
                .await;
        }
        if decision.matches.is_empty() {
            return;
        }
        let synthetic_message = Message {
            id: event_message_id,
            account_id: account_id.into(),
            group_id,
            server_message_id: String::new(),
            sequence: 0,
            user_id: member.user_id,
            sender_name: member.card_name.clone(),
            kind: "member_updated".into(),
            text: String::new(),
            sent_at: self.clock.now_utc(),
            received_at: self.clock.now_utc(),
            processed_at: None,
            acknowledged_at: None,
            processing_state: "processed".into(),
            attempts: 1,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: Some("member-event".into()),
            flow: None,
        };
        let skipped_actions = decision
            .action_intents
            .iter()
            .filter(|intent| intent.action.kind == "recall")
            .map(|intent| intent.action.kind.clone())
            .collect::<Vec<_>>();
        let member_event_actions = decision
            .action_intents
            .iter()
            .filter(|intent| intent.action.kind != "recall")
            .cloned()
            .collect::<Vec<_>>();
        let skip_reason = if skipped_actions.is_empty() {
            ""
        } else {
            "成员更新事件没有可撤回的消息"
        };
        let sender_id = self
            .gateway
            .session_identity()
            .await
            .map(|value| value.0)
            .unwrap_or(0);
        if !member_event_actions.is_empty() {
            self.execute_rule_actions(
                Some(app),
                account_id,
                sender_id,
                &synthetic_message,
                &member_event_actions,
                decision.automatic,
            )
            .await;
        }
        let _ = self
            .database
            .record_audit(AuditEvent {
                id: 0,
                account_id: account_id.into(),
                group_id,
                user_id: member.user_id,
                actor: "DH BOT".into(),
                event: "machine_rule_evaluated".into(),
                level: "info".into(),
                details: serde_json::json!({
                    "memberEventId": durable_event_id,
                    "violationCount": member.violation_count,
                    "matchedRuleIds": decision.matches.iter().map(|matched| matched.rule_id).collect::<Vec<_>>(),
                    "skippedActions": skipped_actions,
                    "skipReason": skip_reason,
                    "automatic": decision.automatic,
                    "elapsedMs": elapsed_ms,
                })
                .to_string(),
                created_at: Utc::now(),
            })
            .await;
    }

    async fn enqueue_member_card_job(
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
        } else if let Some(group_id) = value.get("groupId").and_then(value_i64) {
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
        let nim_sender = value.get("from").and_then(Value::as_str);
        let mut user_id = decoded.pointer("/from/id").and_then(value_i64).unwrap_or(0);
        if user_id <= 0 {
            if let Some(nim_id) = nim_sender {
                user_id = self
                    .database
                    .resolve_member_user_id(account_id.to_string(), group_id, nim_id.to_string())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                if user_id <= 0 {
                    user_id = synthetic_nim_user_id(nim_id);
                }
            }
        }
        if user_id <= 0 {
            return Err("发送者身份未识别".into());
        }
        if user_id == sender_id {
            return Err("已忽略本账号消息".into());
        }
        let wire_sender_name = decoded
            .pointer("/from/name")
            .and_then(Value::as_str)
            .or_else(|| value.get("fromNick").and_then(Value::as_str))
            .unwrap_or_default();
        let sender_name = self
            .database
            .list_members(account_id.to_string(), group_id)
            .await
            .ok()
            .and_then(|members| {
                members
                    .into_iter()
                    .find(|member| member.user_id == user_id)
                    .and_then(|member| {
                        preferred_member_display_name(
                            &member.card_name,
                            &member.managed_card_name,
                            &member.nickname,
                        )
                    })
            })
            .unwrap_or_else(|| wire_sender_name.to_string());
        let text = normalized_message_text(decoded, value);
        let format = decoded
            .get("msgFormat")
            .and_then(value_i64)
            .or_else(|| value.get("msgFormat").and_then(value_i64))
            .unwrap_or(99);
        let kind = normalized_message_kind(
            value.get("type").and_then(Value::as_str),
            format,
            !text.trim().is_empty(),
        );
        let now = self.clock.now_utc();
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
                    .unwrap_or_else(|| {
                        let session = value
                            .get("listenerSession")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown-session");
                        format!("seq-{}-{sequence}", session_hash(session))
                    }),
                sequence: sequence as i64,
                user_id,
                sender_name,
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
                    .filter(|value| !value.is_null())
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

    #[allow(clippy::too_many_arguments)]
    async fn process_job(
        &self,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
        job: IncomingJob,
        ai_sender: &mpsc::Sender<AiReplyJob>,
        ai_rule_sender: &mpsc::Sender<AiRuleJob>,
        work_id: &str,
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
                    details: serde_json::json!({
                        "messageId": job.message.id,
                        "serverMessageId": job.message.server_message_id,
                        "sequence": job.sequence,
                        "processingState": "ignored",
                        "reason": "托盘已暂停全部自动化；消息仍已保存"
                    })
                    .to_string(),
                    created_at: Utc::now(),
                })
                .await;
            return Ok(());
        }
        let Some(context) = self
            .database
            .message_processing_context(
                account_id.to_string(),
                job.message.group_id,
                job.message.user_id,
            )
            .await?
        else {
            return Ok(());
        };
        let group = context.group;
        let member = context
            .member
            .unwrap_or_else(|| placeholder_member(account_id, &job.message));
        self.coordination.tracker.progress(work_id, 2, 4);
        if member.join_source == "message-discovered" {
            let _ = self.database.upsert_member(member.clone()).await;
        }
        let recent_messages = context.recent_messages;
        let recent_events = recent_messages
            .iter()
            .map(|message| moderation::RecentEvent {
                user_id: message.user_id,
                kind: message.kind.clone(),
                at: message.sent_at,
            })
            .collect::<Vec<_>>();
        let mut deferred_ai_rules = Vec::new();
        if group.machine_rules_enabled || group.ai_rules_enabled {
            let rules = context.rules;
            let input = moderation::ModerationInput {
                member: &member,
                kind: &job.message.kind,
                text: &job.message.text,
                now: self.clock.now_utc(),
                recent: &recent_events,
                rename_violations: member.violation_count,
            };
            if group.machine_rules_enabled {
                let machine_rules = rules
                    .iter()
                    .filter(|rule| !rule.is_ai_rule() && rule.enabled)
                    .cloned()
                    .collect::<Vec<_>>();
                let started = std::time::Instant::now();
                let decision = moderation::evaluate_with_classifier(
                    &machine_rules,
                    &input,
                    self.semantic_classifier.as_ref(),
                );
                let elapsed_ms = started.elapsed().as_millis() as i64;
                let matched_ids = decision
                    .matches
                    .iter()
                    .map(|matched| matched.rule_id)
                    .collect::<std::collections::HashSet<_>>();
                for rule in &machine_rules {
                    let matched = decision
                        .matches
                        .iter()
                        .find(|matched| matched.rule_id == rule.id);
                    let _ = self
                        .database
                        .record_rule_evaluation(
                            account_id.into(),
                            group.group_id,
                            member.user_id,
                            job.message.id,
                            rule.id,
                            "machine".into(),
                            matched.is_some(),
                            None,
                            rule.mode.clone(),
                            if matched.is_some() {
                                "命中".into()
                            } else {
                                "未命中".into()
                            },
                            matched
                                .map(|value| value.reason.clone())
                                .unwrap_or_default(),
                            elapsed_ms,
                        )
                        .await;
                }
                if !matched_ids.is_empty() {
                    let _ = self.database.record_audit(AuditEvent { id:0, account_id:account_id.into(), group_id:group.group_id, user_id:member.user_id, actor:"DH BOT".into(), event:"machine_rule_evaluated".into(), level:"info".into(), details:serde_json::json!({"messageId":job.message.id,"matchedRuleIds":matched_ids,"automatic":decision.automatic,"elapsedMs":elapsed_ms}).to_string(), created_at:Utc::now() }).await;
                }
                self.execute_rule_actions(
                    Some(app),
                    account_id,
                    sender_id,
                    &job.message,
                    &decision.action_intents,
                    decision.automatic,
                )
                .await;
            }

            if group.ai_rules_enabled
                && group.enabled
                && !group.manual_takeover
                && message_supports_ai_rules(&job.message.kind, &job.message.text)
            {
                deferred_ai_rules = rules
                    .iter()
                    .filter(|rule| rule.is_ai_rule() && rule.enabled)
                    .cloned()
                    .collect::<Vec<_>>();
            }
        }
        self.coordination.tracker.progress(work_id, 3, 4);
        let ai_reply_allowed = self
            .ai_permission(account_id, group.group_id, "reply", group.ai_enabled)
            .await;
        let mut ai_reply_queued = false;
        if group.enabled
            && group.ai_enabled
            && ai_reply_allowed
            && !group.manual_takeover
            && message_explicitly_mentions(&job.message, account_id, sender_id)
        {
            let ai_work_id = runtime_lane_id("aiReply", account_id, group.group_id);
            self.coordination
                .tracker
                .enqueue(&ai_work_id, "aiReply", "生成 @DH 回复", "群聊 AI");
            match ai_sender.try_send(AiReplyJob {
                message: job.message.clone(),
            }) {
                Ok(()) => ai_reply_queued = true,
                Err(error) => {
                    let reason = match error {
                        mpsc::error::TrySendError::Full(_) => "群 AI 队列已满",
                        mpsc::error::TrySendError::Closed(_) => "群 AI 队列已关闭",
                    };
                    self.coordination
                        .tracker
                        .reject_enqueue(&ai_work_id, reason);
                    let _ = self
                        .database
                        .record_audit(AuditEvent {
                            id: 0,
                            account_id: account_id.into(),
                            group_id: group.group_id,
                            user_id: member.user_id,
                            actor: "DH BOT".into(),
                            event: "ai_reply_queue_failed".into(),
                            level: "warning".into(),
                            details: serde_json::json!({
                                "messageId": job.message.id,
                                "status": "failed",
                                "reason": reason,
                            })
                            .to_string(),
                            created_at: Utc::now(),
                        })
                        .await;
                }
            }
        }
        if !deferred_ai_rules.is_empty() {
            let ai_rule_work_id = runtime_lane_id("aiRule", account_id, group.group_id);
            self.coordination.tracker.enqueue(
                &ai_rule_work_id,
                "aiRule",
                "AI 规则判断",
                "群聊语义分类",
            );
            if let Err(error) = ai_rule_sender.try_send(AiRuleJob {
                account_id: account_id.to_string(),
                sender_id,
                group: group.clone(),
                member: member.clone(),
                message: job.message.clone(),
                recent_events: recent_events.clone(),
                rules: deferred_ai_rules,
            }) {
                let reason = match error {
                    mpsc::error::TrySendError::Full(_) => "AI 规则队列已满",
                    mpsc::error::TrySendError::Closed(_) => "AI 规则队列已关闭",
                };
                self.coordination
                    .tracker
                    .reject_enqueue(&ai_rule_work_id, reason);
                let _ = self
                    .database
                    .record_audit(AuditEvent {
                        id: 0,
                        account_id: account_id.into(),
                        group_id: group.group_id,
                        user_id: member.user_id,
                        actor: "DH BOT".into(),
                        event: "ai_rule_queue_rejected".into(),
                        level: "warning".into(),
                        details: serde_json::json!({
                            "messageId": job.message.id,
                            "status": "failed",
                            "error": reason,
                        })
                        .to_string(),
                        created_at: Utc::now(),
                    })
                    .await;
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
                details: message_audit_details(
                    &job.message,
                    job.sequence,
                    "processed",
                    if ai_reply_queued {
                        "deterministic-rules-complete-ai-queued"
                    } else {
                        "deterministic-rules-complete"
                    },
                ),
                created_at: Utc::now(),
            })
            .await;
        self.coordination.tracker.progress(work_id, 4, 4);
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

    async fn process_deferred_ai(
        &self,
        app: &AppHandle,
        account_id: &str,
        sender_id: i64,
        message: Message,
    ) -> AppResult<()> {
        if self
            .database
            .get_setting("automation.mode".into())
            .await?
            .as_deref()
            == Some("paused")
        {
            return Ok(());
        }
        let Some(group) = self
            .database
            .list_groups(Some(account_id.to_string()))
            .await?
            .into_iter()
            .find(|group| group.group_id == message.group_id)
        else {
            return Ok(());
        };
        if !group.enabled
            || !group.ai_enabled
            || group.manual_takeover
            || !self
                .ai_permission(account_id, group.group_id, "reply", group.ai_enabled)
                .await
            || !message_explicitly_mentions(&message, account_id, sender_id)
        {
            return Ok(());
        }
        let member = self
            .database
            .list_members(account_id.to_string(), group.group_id)
            .await?
            .into_iter()
            .find(|member| member.user_id == message.user_id)
            .unwrap_or_else(|| placeholder_member(account_id, &message));
        if let Some(business_app) = self.business_apps.find_for_message(&message.text) {
            let app_id = business_app.manifest().id.to_string();
            if self.is_group_manager(sender_id, group.group_id).await
                && self
                    .database
                    .business_app_enabled(account_id.to_string(), app_id)
                    .await?
            {
                self.process_business_app(account_id, &group, &member, &message, business_app)
                    .await?;
            }
            return Ok(());
        }
        self.process_ai(app, account_id, &group, &member, &message)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_ai_rules_background(
        &self,
        app: AppHandle,
        account_id: String,
        sender_id: i64,
        group: Group,
        member: Member,
        message: Message,
        recent_events: Vec<moderation::RecentEvent>,
        ai_rules: Vec<ModerationRule>,
    ) {
        let work_id = runtime_lane_id("aiRule", &account_id, group.group_id);
        self.coordination.tracker.start_named(
            &work_id,
            "aiRule",
            "AI 规则判断",
            "群聊语义分类",
            None,
        );
        let mut categories = ai_rules
            .iter()
            .map(|rule| rule.pattern.trim().to_string())
            .filter(|category| !category.is_empty())
            .collect::<Vec<_>>();
        categories.sort();
        categories.dedup();
        if categories.is_empty() {
            self.coordination.tracker.finish(&work_id, "succeeded", "");
            return;
        }
        let input = moderation::ModerationInput {
            member: &member,
            kind: &message.kind,
            text: &message.text,
            now: self.clock.now_utc(),
            recent: &recent_events,
            rename_violations: member.violation_count,
        };
        let started = std::time::Instant::now();
        match self
            .classify_semantics(&group, &member, &message, &categories)
            .await
        {
            Ok(scores) => {
                let decision =
                    moderation::evaluate_with_semantic_scores(&ai_rules, &input, &scores);
                let elapsed_ms = started.elapsed().as_millis() as i64;
                for rule in &ai_rules {
                    let matched = decision
                        .matches
                        .iter()
                        .find(|matched| matched.rule_id == rule.id);
                    let confidence = scores.get(rule.pattern.trim()).copied();
                    let _ = self
                        .database
                        .record_rule_evaluation(
                            account_id.clone(),
                            group.group_id,
                            member.user_id,
                            message.id,
                            rule.id,
                            "ai".into(),
                            matched.is_some(),
                            confidence,
                            rule.mode.clone(),
                            if matched.is_some() {
                                "命中".into()
                            } else {
                                "未命中".into()
                            },
                            matched
                                .map(|value| value.reason.clone())
                                .unwrap_or_default(),
                            elapsed_ms,
                        )
                        .await;
                }
                let allow_recall = self
                    .ai_permission(&account_id, group.group_id, "recall", false)
                    .await;
                let allow_mute = self
                    .ai_permission(&account_id, group.group_id, "mute", false)
                    .await;
                let allow_remove = self
                    .ai_permission(&account_id, group.group_id, "remove", false)
                    .await;
                let allow_notify = self
                    .ai_permission(&account_id, group.group_id, "reply", false)
                    .await;
                let action_intents = filter_ai_rule_intents(
                    decision.action_intents,
                    allow_recall,
                    allow_mute,
                    allow_remove,
                    allow_notify,
                );
                let matched_rule_ids = decision
                    .matches
                    .iter()
                    .map(|matched| matched.rule_id)
                    .collect::<Vec<_>>();
                let automatic_action_rule_ids = action_intents
                    .iter()
                    .flat_map(|intent| intent.contributors.iter().map(|value| value.rule_id))
                    .collect::<Vec<_>>();
                let _ = self.database.record_audit(AuditEvent { id:0, account_id:account_id.clone(), group_id:group.group_id, user_id:member.user_id, actor:"DH BOT".into(), event:"ai_rule_evaluated".into(), level:"info".into(), details:serde_json::json!({"messageId":message.id,"scores":scores,"matchedRuleIds":matched_rule_ids,"automaticActionRuleIds":automatic_action_rule_ids,"elapsedMs":elapsed_ms,"execution":"background"}).to_string(), created_at:Utc::now() }).await;
                self.execute_rule_actions(
                    Some(&app),
                    &account_id,
                    sender_id,
                    &message,
                    &action_intents,
                    decision.automatic,
                )
                .await;
                self.coordination.tracker.finish(&work_id, "succeeded", "");
            }
            Err(error) => {
                self.coordination
                    .tracker
                    .finish(&work_id, "failed", &error.message);
                let _ = self.database.record_audit(AuditEvent { id:0, account_id, group_id:group.group_id, user_id:member.user_id, actor:"DH BOT".into(), event:"ai_rule_failed".into(), level:"warning".into(), details:serde_json::json!({"messageId":message.id,"error":error.message,"decision":"未执行，不使用本地猜测结果","execution":"background"}).to_string(), created_at:Utc::now() }).await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_rule_actions(
        &self,
        app: Option<&AppHandle>,
        account_id: &str,
        sender_id: i64,
        message: &Message,
        intents: &[moderation::ActionIntent],
        automatic: bool,
    ) {
        if !automatic {
            return;
        }
        if self.gateway.calibration_active() {
            self.logger.write("INFO", "真实校准期间跳过自动规则副作用");
            return;
        }
        let permission = self
            .gateway
            .list_members(message.group_id)
            .await
            .ok()
            .and_then(|roster| {
                roster.members.into_iter().find(|member| {
                    member.user_id == sender_id && member.user_id > 0 && member.present
                })
            })
            .map(|member| member.role == "owner" || member.role == "admin")
            .unwrap_or(false);
        let mut ordered = intents.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|intent| match intent.action.kind.as_str() {
            "recall" => 0,
            "remove" | "mute" | "unmute" => 1,
            "blacklist" => 2,
            "notify" | "reply" => 3,
            _ => 4,
        });
        for intent in ordered {
            let action = &intent.action;
            let mut contributor_rule_ids = intent
                .contributors
                .iter()
                .map(|value| value.rule_id)
                .collect::<Vec<_>>();
            contributor_rule_ids.sort_unstable();
            contributor_rule_ids.dedup();
            let rule_id = intent.contributors.first().map(|value| value.rule_id);
            // Machine and AI rules may independently request the same action for one
            // message. The external side effect is still performed only once.
            let dedupe_key = format!("message:{}:action:{}", message.id, action.kind);
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
                                "reason":if action.message.is_empty() { intent.contributors.iter().map(|value| value.rule_name.as_str()).collect::<Vec<_>>().join("、") } else { action.message.clone() },
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
                        if let Some(app) = app {
                            let _ = app.emit(
                                "action-recorded",
                                serde_json::json!({"kind":action.kind,"state":"queued"}),
                            );
                        }
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
            if let Some(app) = app {
                let _ = app.emit(
                    "action-recorded",
                    json_action(&action.kind, false, &error_text),
                );
            }
        }
    }

    async fn classify_semantics(
        &self,
        group: &Group,
        member: &Member,
        message: &Message,
        categories: &[String],
    ) -> AppResult<HashMap<String, f64>> {
        let (provider, _) = self
            .ai_provider(&group.account_id, ai::AI_PROVIDER_TIMEOUT)
            .await?;
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
                "@DH 这是内部语义分类请求，不是群聊回复。\n类别={category_text}\n待分类消息={}\n按统一 AI 响应协议返回；reply 必须是字符串，字符串内容只包含一个 JSON 对象，键为类别，值为 0 到 1 的置信度。actions 和 tasks 必须为空。",
                message.text
            ),
            recent_context: Vec::new(),
            knowledge: Vec::new(),
        };
        let decision = provider.decide(&request).await?;
        parse_semantic_scores(&decision.reply, categories)
            .or_else(|reply_error| {
                parse_semantic_scores(&decision.reason, categories).map_err(|_| reply_error)
            })
            .map_err(|error| {
                let reply_preview = semantic_response_preview(&decision.reply);
                let reason_preview = semantic_response_preview(&decision.reason);
                AppError::new(
                    error.code,
                    format!(
                        "{}；reply={}；reason={}",
                        error.message, reply_preview, reason_preview
                    ),
                )
            })
    }

    async fn process_business_app(
        &self,
        account_id: &str,
        group: &Group,
        member: &Member,
        message: &Message,
        app: Arc<dyn BusinessApp>,
    ) -> AppResult<()> {
        let manifest = app.manifest();
        let run_key = format!("message:{}", message.id);
        let started = std::time::Instant::now();
        let claimed = self
            .database
            .claim_business_app_run(crate::models::BusinessAppRun {
                id: 0,
                account_id: account_id.into(),
                app_id: manifest.id.into(),
                group_id: group.group_id,
                message_id: message.id,
                run_key: run_key.clone(),
                status: "processing".into(),
                freshness: "missing".into(),
                ai_used: false,
                reply: String::new(),
                error: String::new(),
                elapsed_ms: 0,
                created_at: self.clock.now_utc(),
                completed_at: None,
            })
            .await?;
        if !claimed {
            return Ok(());
        }

        let result: AppResult<()> = async {
            let outcome = app
                .run(&BusinessAppContext {
                    account_id,
                    group,
                    member,
                    message,
                    now: self.clock.now_utc(),
                })
                .await?;
            if outcome.app_id != manifest.id {
                return Err(AppError::new(
                    "business_app_result",
                    "业务应用返回了错误的应用标识",
                ));
            }
            // 群内预测先返回确定性模板，避免额外等待一次模型生成。
            // AI 润色由本地测试或新期开奖预热任务完成，不阻塞本次群回复。
            let mut reply = outcome.fallback_reply.clone();
            let mut ai_used = false;
            let mut ai_error = String::new();
            if let Some(narration) = outcome.narration.as_ref() {
                match self.ai_provider(account_id, ai::AI_PROVIDER_TIMEOUT).await {
                    Ok((provider, provider_revision)) => {
                        let cache_key = ai::prediction_narration_cache_key(
                            account_id,
                            manifest.version,
                            narration,
                            &provider_revision,
                        );
                        if let Some(decision) = self
                            .prediction_narration_cache
                            .get_scoped(account_id, &cache_key)
                        {
                            if crate::business_apps::validate_prediction_narration(
                                &decision.reply,
                                narration,
                            ) {
                                reply = decision.reply;
                                ai_used = true;
                            } else {
                                self.prediction_narration_cache
                                    .remove_scoped(account_id, &cache_key);
                                ai_error = "AI 润色字段不完整，已使用即时统计模板".into();
                            }
                        } else {
                            let request = AiRequest {
                                version: "1",
                                event_id: uuid::Uuid::new_v4().to_string(),
                                persona: ai::PERSONA,
                                group_id: group.group_id,
                                group_name: group.name.clone(),
                                member_id: member.user_id,
                                member_name: member.card_name.clone(),
                                member_role: member.role.clone(),
                                message_id: format!("prediction:{cache_key}"),
                                message: crate::business_apps::prediction_narration_prompt(
                                    narration,
                                ),
                                recent_context: Vec::new(),
                                knowledge: Vec::new(),
                            };
                            let should_queue = self
                                .prediction_narration_pending
                                .lock()
                                .await
                                .insert(cache_key.clone());
                            if should_queue {
                                let work_id =
                                    runtime_lane_id("prediction", account_id, group.group_id);
                                self.coordination.tracker.enqueue(
                                    &work_id,
                                    "prediction",
                                    "生成预测说明",
                                    "业务应用",
                                );
                                match self.prediction_narration_tx.try_send(
                                    PredictionNarrationJob {
                                        account_id: account_id.into(),
                                        group_id: group.group_id,
                                        cache_key: cache_key.clone(),
                                        narration: narration.clone(),
                                        provider,
                                        request,
                                    },
                                ) {
                                    Ok(()) => {
                                        ai_error = "已返回即时统计模板；AI 润色正在后台预热".into();
                                    }
                                    Err(_) => {
                                        self.coordination
                                            .tracker
                                            .reject_enqueue(&work_id, "AI 预测说明队列繁忙");
                                        self.prediction_narration_pending
                                            .lock()
                                            .await
                                            .remove(&cache_key);
                                        ai_error = "已返回即时统计模板；AI 预热队列繁忙".into();
                                    }
                                }
                            } else {
                                ai_error = "已返回即时统计模板；AI 润色正在后台预热".into();
                            }
                        }
                    }
                    Err(_) => {
                        ai_error = "已返回即时统计模板；AI 连接尚未可用".into();
                    }
                }
            }

            if !reply.trim().is_empty() {
                self.enqueue_text_effect(
                    account_id,
                    group.group_id,
                    &reply,
                    "business-app-reply",
                    format!("business-app:{}:{}", manifest.id, message.id),
                    serde_json::json!({
                        "appId": manifest.id,
                        "messageId": message.id,
                        "userId": message.user_id,
                    }),
                )
                .await?;
            }
            let final_status = if !ai_error.is_empty() && outcome.status == "succeeded" {
                "fallback"
            } else {
                outcome.status.as_str()
            };
            self.database
                .finish_business_app_run(
                    account_id.into(),
                    manifest.id.into(),
                    run_key.clone(),
                    final_status.into(),
                    outcome.freshness,
                    ai_used,
                    reply,
                    ai_error,
                    started.elapsed().as_millis().min(i64::MAX as u128) as i64,
                )
                .await?;
            Ok(())
        }
        .await;

        if let Err(error) = &result {
            let _ = self
                .database
                .finish_business_app_run(
                    account_id.into(),
                    manifest.id.into(),
                    run_key,
                    "failed".into(),
                    "missing".into(),
                    false,
                    String::new(),
                    error.message.clone(),
                    started.elapsed().as_millis().min(i64::MAX as u128) as i64,
                )
                .await;
        }
        result
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
            let total_started = std::time::Instant::now();
            let queue_ms = self
                .clock
                .now_utc()
                .signed_duration_since(message.received_at)
                .num_milliseconds()
                .max(0);
            let selected_endpoint = self
                .database
                .list_ai_provider_endpoints(account_id.to_string())
                .await?
                .into_iter()
                .filter(|endpoint| endpoint.enabled)
                .min_by_key(|endpoint| (endpoint.priority, endpoint.id));
            let (provider, provider_revision) = self
                .ai_provider(account_id, ai::AI_PROVIDER_TIMEOUT)
                .await?;
            let knowledge_started = std::time::Instant::now();
            let documents = self
                .database
                .list_knowledge_for_group(account_id.to_string(), group.group_id)
                .await?;
            let (active_activities, _) = self
                .database
                .activity_context(account_id.to_string(), group.group_id, 10)
                .await
                .unwrap_or_default();
            let mut knowledge_revision = Sha256::new();
            for document in &documents {
                knowledge_revision.update(document.id.to_le_bytes());
                knowledge_revision.update(document.content_hash.as_bytes());
                knowledge_revision.update([0]);
            }
            for activity in &active_activities {
                knowledge_revision.update(activity.id.to_le_bytes());
                knowledge_revision.update(activity.updated_at.to_rfc3339().as_bytes());
                knowledge_revision.update([0]);
            }
            let knowledge_revision = format!("{:x}", knowledge_revision.finalize());
            let question = ai::message_without_mention(&message.text);
            let knowledge_cache_key =
                ai::knowledge_cache_key(account_id, group.group_id, &question, &knowledge_revision);
            let (knowledge_chunks, knowledge_cache_hit) =
                if let Some(chunks) = self.ai_knowledge_cache.get(&knowledge_cache_key) {
                    (chunks, true)
                } else {
                    let mut chunks = knowledge::search(&question, &documents, 3)
                        .into_iter()
                        .map(|hit| ai::AiKnowledgeChunk {
                            base: hit.document.base_name,
                            title: hit.document.title,
                            source: hit.document.source,
                            text: hit.excerpt,
                        })
                        .collect::<Vec<_>>();
                    chunks.extend(active_activities.iter().map(|activity| {
                        ai::AiKnowledgeChunk {
                            base: "当前群活动".into(),
                            title: activity.name.clone(),
                            source: "activity".into(),
                            text: format!(
                                "活动日期：{} 至 {}；发送时刻：{}；活动内容：{}",
                                activity.start_date,
                                activity.end_date,
                                activity.send_times.join("、"),
                                activity.content
                            ),
                        }
                    }));
                    self.ai_knowledge_cache
                        .insert(knowledge_cache_key, chunks.clone());
                    (chunks, false)
                };
            let knowledge_ms = knowledge_started.elapsed().as_millis();
            let recent_messages = self
                .database
                .recent_messages(account_id.to_string(), group.group_id, 9)
                .await?
                .into_iter()
                .filter(|candidate| candidate.id != message.id)
                .collect::<Vec<_>>();
            let recent_context = if !knowledge_chunks.is_empty() && ai::is_cacheable_faq(&question)
            {
                Vec::new()
            } else {
                ai::build_recent_context(&recent_messages, 8)
            };
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
                // The runtime has already verified either @DH text or WangShangLiao
                // mention metadata. Preserve that fact for the persona after the
                // visible mention is stripped from the retrieval query.
                message: format!("@DH {question}"),
                recent_context,
                knowledge: knowledge_chunks,
            };
            let cache_key = ai::answer_cache_key(
                account_id,
                group.group_id,
                &question,
                &knowledge_revision,
                &provider_revision,
            );
            let cacheable = !request.knowledge.is_empty()
                && request.recent_context.is_empty()
                && ai::is_cacheable_faq(&question);
            let (decision, cache_hit, cache_stored, model_ms) = if cacheable {
                if let Some(decision) = self.ai_reply_cache.get_scoped(account_id, &cache_key) {
                    (decision, true, true, 0)
                } else {
                    let model_started = std::time::Instant::now();
                    let decision = provider.decide(&request).await?;
                    let model_ms = model_started.elapsed().as_millis();
                    let cache_stored = self.ai_reply_cache
                        .insert_scoped(account_id, cache_key, decision.clone());
                    (decision, false, cache_stored, model_ms)
                }
            } else {
                let model_started = std::time::Instant::now();
                let decision = provider.decide(&request).await?;
                (decision, false, false, model_started.elapsed().as_millis())
            };
            let decision_action_count = decision.actions.len();
            let decision_task_count = decision.tasks.len();
            let provider_attempts = if cache_hit {
                0
            } else {
                provider.last_attempt_count()
            };
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
            let allow_mute = self
                .ai_permission(account_id, group.group_id, "mute", false)
                .await;
            let allow_remove = self
                .ai_permission(account_id, group.group_id, "remove", false)
                .await;
            let actions = filter_ai_actions(decision.actions, allow_mute, allow_remove);
            let action_intents = actions
                .into_iter()
                .map(|action| moderation::ActionIntent {
                    action,
                    contributors: Vec::new(),
                })
                .collect::<Vec<_>>();
            if !action_intents.is_empty() {
                self.execute_rule_actions(
                    Some(app),
                    account_id,
                    self.gateway.session_identity().await?.0,
                    message,
                    &action_intents,
                    true,
                )
                .await;
            }
            if self
                .ai_permission(account_id, group.group_id, "tasks", true)
                .await
            {
                for (index, task) in decision.tasks.into_iter().enumerate() {
                    let scheduled = task
                        .due_at
                        .unwrap_or_else(|| Utc::now() + chrono::Duration::days(1))
                        .with_timezone(&Local);
                    let title = task.title.trim().to_string();
                    let content = if task.description.trim().is_empty() {
                        title.clone()
                    } else {
                        task.description.trim().to_string()
                    };
                    let _ = self
                        .database
                        .save_activity_once(
                            Activity {
                                id: 0,
                                account_id: account_id.into(),
                                name: title,
                                content,
                                enabled: false,
                                ai_optimize: false,
                                ai_instructions: String::new(),
                                timezone: iana_time_zone::get_timezone()
                                    .unwrap_or_else(|_| "UTC".into()),
                                start_date: scheduled.format("%Y-%m-%d").to_string(),
                                end_date: scheduled.format("%Y-%m-%d").to_string(),
                                weekdays: vec![1, 2, 3, 4, 5, 6, 7],
                                send_times: vec![scheduled.format("%H:%M").to_string()],
                                group_ids: vec![group.group_id],
                                next_run_at: None,
                                source_key: String::new(),
                                deleted_at: None,
                                created_at: Utc::now(),
                                updated_at: Utc::now(),
                            },
                            format!("ai-task:{}:{index}", message.id),
                        )
                        .await;
                }
            }
            let _ = self
                .database
                .record_audit(AuditEvent {
                    id: 0,
                    account_id: account_id.into(),
                    group_id: group.group_id,
                    user_id: message.user_id,
                    actor: "DH BOT".into(),
                    event: "ai_reply_completed".into(),
                    level: "info".into(),
                    details: serde_json::json!({
                        "messageId": message.id,
                        "queueMs": queue_ms,
                        "cacheHit": cache_hit,
                        "cacheEligible": cacheable,
                        "cacheStored": cache_stored,
                        "decisionActions": decision_action_count,
                        "decisionTasks": decision_task_count,
                        "knowledgeCacheHit": knowledge_cache_hit,
                        "knowledgeChunks": request.knowledge.len(),
                        "knowledgeMs": knowledge_ms,
                        "modelMs": model_ms,
                        "providerAttempts": provider_attempts,
                        "providerName": selected_endpoint.as_ref().map(|endpoint| endpoint.name.as_str()).unwrap_or(""),
                        "model": selected_endpoint.as_ref().map(|endpoint| endpoint.model.as_str()).unwrap_or(""),
                        "backend": selected_endpoint.as_ref().map(|endpoint| endpoint.api_backend.as_str()).unwrap_or(""),
                        "reasoningEffort": selected_endpoint.as_ref().map(|endpoint| endpoint.reasoning_effort.as_str()).unwrap_or(""),
                        "failover": provider_attempts > 1,
                        "totalMs": total_started.elapsed().as_millis(),
                        "status": "succeeded"
                    })
                    .to_string(),
                    created_at: Utc::now(),
                })
                .await;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                let _ = self
                    .database
                    .finish_ai_run(account_id.to_string(), run_key, true, String::new())
                    .await;
                Ok(())
            }
            Err(error) => {
                let _ = self
                    .database
                    .record_audit(AuditEvent {
                        id: 0,
                        account_id: account_id.into(),
                        group_id: group.group_id,
                        user_id: message.user_id,
                        actor: "DH BOT".into(),
                        event: "ai_reply_failed".into(),
                        level: "warning".into(),
                        details: serde_json::json!({
                            "messageId": message.id,
                            "status": "failed",
                            "error": error.message,
                            "fallback": "ai-service-unavailable"
                        })
                        .to_string(),
                        created_at: Utc::now(),
                    })
                    .await;
                let fallback = self
                    .enqueue_text_effect(
                        account_id,
                        group.group_id,
                        "AI 服务暂时连接不上，请稍后再试。",
                        "ai-reply-fallback",
                        format!("ai-reply-fallback:{}", message.id),
                        serde_json::json!({
                            "messageId": message.id,
                            "userId": message.user_id,
                            "providerError": true,
                        }),
                    )
                    .await;
                let _ = self
                    .database
                    .fail_ai_run_terminal(account_id.to_string(), run_key, error.message.clone())
                    .await;
                fallback
            }
        }
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

    async fn ai_configs(
        &self,
        account_id: &str,
        timeout: Duration,
    ) -> AppResult<(Vec<AiConfig>, String)> {
        self.database
            .ensure_ai_provider_endpoints(account_id.to_string())
            .await?;
        let endpoints = self
            .database
            .list_ai_provider_endpoints(account_id.to_string())
            .await?;
        let secrets = self.secrets.load().unwrap_or_default();
        let mut revision = Sha256::new();
        let configs = endpoints
            .iter()
            .filter(|endpoint| endpoint.enabled)
            .map(|endpoint| {
                revision.update(endpoint.id.to_le_bytes());
                revision.update(endpoint.updated_at.to_rfc3339().as_bytes());
                revision.update([0]);
                AiConfig {
                    base_url: endpoint.base_url.clone(),
                    webhook_url: endpoint.webhook_url.clone(),
                    api_backend: endpoint.api_backend.clone(),
                    model: endpoint.model.clone(),
                    reasoning_effort: endpoint.reasoning_effort.clone(),
                    api_key: secrets
                        .get(&endpoint.secret_ref)
                        .cloned()
                        .unwrap_or_default(),
                    timeout,
                }
            })
            .collect::<Vec<_>>();
        Ok((configs, format!("{:x}", revision.finalize())))
    }

    async fn ai_provider(
        &self,
        account_id: &str,
        timeout: Duration,
    ) -> AppResult<(Arc<dyn AiProvider>, String)> {
        let (configs, revision) = self.ai_configs(account_id, timeout).await?;
        Ok((self.ai_factory.create(configs)?, revision))
    }

    #[cfg(feature = "fixture")]
    pub async fn headless_ingest_once(&self) -> AppResult<usize> {
        let diagnostic = self.gateway.diagnose().await;
        if diagnostic.status != ConnectionStatus::Ready {
            return Err(AppError::new(
                "headless_gateway_not_ready",
                "Headless 运行时等待协议会话就绪",
            ));
        }
        let (sender_id, account_id) = self.gateway.session_identity().await?;
        let now = self.clock.now_utc();
        self.database
            .upsert_account(Account {
                id: account_id.clone(),
                display_name: diagnostic.nim_account,
                role: "unknown".into(),
                discovered_at: now,
                updated_at: now,
            })
            .await?;
        let groups = self.gateway.list_groups().await?;
        for mut group in groups {
            group.account_id = account_id.clone();
            let group_id = group.group_id;
            self.database.upsert_group(group).await?;
            let roster = self.gateway.list_members(group_id).await?;
            for mut member in roster.members {
                member.account_id = account_id.clone();
                member.group_id = group_id;
                self.database.upsert_member(member).await?;
            }
        }
        self.gateway.install_message_listener().await?;
        let batch = self.gateway.read_batch().await?;
        if batch.records.is_empty() {
            return Ok(0);
        }
        let inbox = batch
            .records
            .iter()
            .map(|record| GatewayInboxEvent {
                account_id: account_id.clone(),
                bridge_session: record.session.clone(),
                bridge_sequence: record.sequence as i64,
                event_id: gateway_record_id(record),
                event_type: "message".into(),
                payload_json: record.payload.to_string(),
                received_at: self.clock.now_utc(),
            })
            .collect::<Vec<_>>();
        let mut normalized = Vec::new();
        let mut ackable = HashSet::new();
        for record in &batch.records {
            if record.kind != GatewayRecordKind::Message {
                continue;
            }
            let mut raw = record.payload.clone();
            if let Some(object) = raw.as_object_mut() {
                object.insert("seq".into(), Value::from(record.sequence));
                object.insert("source".into(), Value::from(record.source.clone()));
                object.insert(
                    "listenerSession".into(),
                    Value::from(record.session.clone()),
                );
            }
            match self.normalize_message(&account_id, sender_id, &raw).await {
                Ok(job) => {
                    ackable.insert(gateway_record_id(record));
                    normalized.push(job);
                }
                Err(reason) => self.events.emit(
                    "headless-message-ignored",
                    serde_json::json!({"reason": reason, "payload": raw}),
                ),
            }
        }
        let (_, persisted) = self
            .database
            .ingest_gateway_batch(
                inbox,
                normalized.iter().map(|job| job.message.clone()).collect(),
            )
            .await?;
        let ack_sequence = contiguous_ack_sequence(&batch.records, &ackable);
        if ack_sequence == 0 {
            return Ok(0);
        }
        self.gateway.ack(&batch.session, ack_sequence).await?;
        self.database
            .commit_gateway_ack(
                account_id.clone(),
                batch
                    .records
                    .iter()
                    .take_while(|record| record.sequence <= ack_sequence)
                    .map(gateway_record_id)
                    .collect(),
                persisted.iter().map(|message| message.id).collect(),
            )
            .await?;
        for (job, persisted) in normalized.into_iter().zip(&persisted) {
            self.enqueue_text_effect(
                &account_id,
                job.message.group_id,
                "DH BOT Headless 回执",
                "headless-e2e",
                format!("headless-e2e:{}", persisted.id),
                serde_json::json!({
                    "recordAction": true,
                    "actionKind": "reply",
                    "messageId": persisted.id,
                    "userId": job.message.user_id,
                    "mode": "automatic",
                    "reason": "CDP headless end-to-end",
                }),
            )
            .await?;
        }
        self.events.emit(
            "message-received",
            serde_json::json!({"count": persisted.len(), "accountId": account_id}),
        );
        Ok(persisted.len())
    }

    #[cfg(feature = "fixture")]
    pub async fn headless_dispatch_once(&self) -> AppResult<usize> {
        let account_id = self.gateway.session_identity().await?.1;
        let mut count = 0;
        while count < 100 {
            let permit = self.gateway.automatic_write_permit().await?;
            let Some(item) = self
                .database
                .claim_effect_outbox(Some(account_id.clone()), 1)
                .await?
                .into_iter()
                .next()
            else {
                break;
            };
            use crate::queue_kernel::ChainOutcome;
            let outcome = self
                .queue_kernel
                .run_chain(item.clone(), account_id.clone())
                .await;
            match outcome {
                ChainOutcome::Skip { reason } => {
                    self.skip_effect_item(&item, &reason).await;
                }
                ChainOutcome::Fail { error } => {
                    self.reject_effect_item(&item, &error, false).await;
                }
                ChainOutcome::Retry { error } => {
                    self.reject_effect_item(&item, &error, true).await;
                }
                ChainOutcome::Proceed {
                    order_guard,
                    lane_permit,
                } => {
                    self.dispatch_effect_permitted(None, item, &permit).await;
                    drop(order_guard);
                    drop(lane_permit);
                }
            }
            count += 1;
        }
        Ok(count)
    }
}

fn message_audit_details(
    message: &Message,
    sequence: u64,
    processing_state: &str,
    result: &str,
) -> String {
    serde_json::json!({
        "direction": "incoming",
        "messageId": message.id,
        "serverMessageId": message.server_message_id,
        "sequence": sequence,
        "kind": message.kind,
        "senderName": message.sender_name,
        "contentPreview": audit_content_preview(&message.text),
        "processingState": processing_state,
        "source": message.source_kind,
        "flow": message.flow,
        "result": result,
    })
    .to_string()
}

fn audit_content_preview(value: &str) -> String {
    let normalized = redact(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = normalized.chars();
    let preview = chars.by_ref().take(120).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn redact_gateway_receipt(mut receipt: GatewayReceipt) -> GatewayReceipt {
    receipt.route = redact(&receipt.route);
    receipt.status = redact(&receipt.status);
    receipt.business_message = redact(&receipt.business_message);
    receipt.request_id = redact(&receipt.request_id);
    receipt.message_id = redact(&receipt.message_id);
    receipt.session = redact(&receipt.session);
    receipt
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
    incoming.join_source = saved.join_source.clone();
    if incoming.joined_at.is_none() {
        incoming.joined_at = saved.joined_at;
    }
}

/// 只有 DH 锁定过群名片的成员，才会因为外部改名累计违规。
/// `has_locked_card_name` 对应该成员存在生效中的 `locked_card_name`；
/// 缺少这个前提会让从未纳入名片管理的普通成员被 `rename_count` 规则处罚。
fn is_external_member_card_update(
    saved_exists: bool,
    has_locked_card_name: bool,
    previous_card_name: &str,
    incoming_card_name: &str,
    expected_update: bool,
) -> bool {
    saved_exists
        && has_locked_card_name
        && !expected_update
        && !incoming_card_name.trim().is_empty()
        && previous_card_name.trim() != incoming_card_name.trim()
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

fn durable_event_numeric_id(value: &str) -> i64 {
    let mut hash: u64 = 14695981039346656037;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    -((hash & 0x3fff_ffff_ffff_ffff) as i64) - 1
}

fn session_hash(session: &str) -> String {
    let digest = Sha256::digest(session.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn contiguous_ack_sequence(records: &[GatewayRecord], ackable_event_ids: &HashSet<String>) -> u64 {
    records
        .iter()
        .take_while(|record| ackable_event_ids.contains(&gateway_record_id(record)))
        .map(|record| record.sequence)
        .last()
        .unwrap_or(0)
}

fn message_explicitly_mentions(message: &Message, account_id: &str, user_id: i64) -> bool {
    if ai::is_mentioned(&message.text) {
        return true;
    }
    let Ok(metadata) = serde_json::from_str::<Value>(&message.mentions_json) else {
        return false;
    };
    let user_id = user_id.to_string();
    fn scalar_matches(value: &Value, account_id: &str, user_id: &str) -> bool {
        match value {
            Value::String(value) => value.eq_ignore_ascii_case(account_id) || value == user_id,
            Value::Number(value) => value.to_string() == account_id || value.to_string() == user_id,
            _ => false,
        }
    }
    fn contains_account(value: &Value, account_id: &str, user_id: &str) -> bool {
        match value {
            Value::String(_) | Value::Number(_) => scalar_matches(value, account_id, user_id),
            Value::Array(values) => values
                .iter()
                .any(|value| contains_account(value, account_id, user_id)),
            Value::Object(values) => values.iter().any(|(key, value)| {
                (matches!(
                    key.as_str(),
                    "account" | "accid" | "nimId" | "accountId" | "target" | "uid"
                ) && scalar_matches(value, account_id, user_id))
                    || (matches!(value, Value::Array(_) | Value::Object(_))
                        && contains_account(value, account_id, user_id))
            }),
            _ => false,
        }
    }
    contains_account(&metadata, account_id, &user_id)
}

fn filter_ai_actions(
    actions: Vec<RuleAction>,
    allow_mute: bool,
    allow_remove: bool,
) -> Vec<RuleAction> {
    actions
        .into_iter()
        .filter(|action| match action.kind.as_str() {
            "mute" | "unmute" => allow_mute,
            "remove" => allow_remove,
            _ => false,
        })
        .collect()
}

fn filter_ai_rule_intents(
    intents: Vec<moderation::ActionIntent>,
    allow_recall: bool,
    allow_mute: bool,
    allow_remove: bool,
    allow_notify: bool,
) -> Vec<moderation::ActionIntent> {
    intents
        .into_iter()
        .filter(|intent| match intent.action.kind.as_str() {
            "recall" => allow_recall,
            "mute" | "unmute" => allow_mute,
            "remove" | "blacklist" => allow_remove,
            "notify" => allow_notify,
            _ => false,
        })
        .collect()
}

fn message_supports_ai_rules(kind: &str, text: &str) -> bool {
    kind == "text" && !text.trim().is_empty()
}

fn preferred_member_display_name(
    card_name: &str,
    managed_card_name: &str,
    nickname: &str,
) -> Option<String> {
    [card_name, managed_card_name, nickname]
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalized_message_kind(raw_type: Option<&str>, format: i64, has_text: bool) -> &'static str {
    match raw_type {
        Some("text") => "text",
        Some("image") => "image",
        Some("card") => "card",
        Some("notification") | Some("notice") => "notice",
        Some("custom") if has_text => "text",
        Some(_) => "other",
        None => match format {
            0 => "text",
            1 => "image",
            13 => "card",
            7 | 8 => "notice",
            _ if has_text => "text",
            _ => "other",
        },
    }
}

fn normalized_message_text(decoded: &Value, raw: &Value) -> String {
    decoded
        .pointer("/content/data")
        .and_then(Value::as_str)
        .or_else(|| {
            decoded
                .pointer("/mentions/content/data")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            decoded
                .pointer("/aite/content/data")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            raw.pointer("/mentions/content/data")
                .and_then(Value::as_str)
        })
        .or_else(|| raw.pointer("/aite/content/data").and_then(Value::as_str))
        .or_else(|| raw.get("text").and_then(Value::as_str))
        .or_else(|| raw.get("content").and_then(Value::as_str))
        .unwrap_or_else(|| {
            if decoded.is_null() {
                "[消息解码失败]"
            } else {
                ""
            }
        })
        .to_string()
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

fn semantic_response_preview(value: &str) -> String {
    let compact = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect::<String>();
    if compact.is_empty() {
        "[空]".into()
    } else {
        compact
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "fixture")]
    use crate::calibration::CalibrationMetadata;
    use crate::database::Database;
    use crate::fixture::{FixtureGateway, FIXTURE_ACCOUNT, FIXTURE_GROUP};
    use crate::gateway::GroupGateway;
    #[cfg(feature = "fixture")]
    use crate::gateway::{CdpClient, CdpGateway};
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

    fn action(kind: &str) -> RuleAction {
        RuleAction {
            kind: kind.into(),
            duration_seconds: 600,
            message: String::new(),
        }
    }

    #[test]
    fn ai_decisions_never_control_recall() {
        let actions = filter_ai_actions(
            vec![action("recall"), action("mute"), action("remove")],
            true,
            true,
        );
        assert_eq!(
            actions
                .iter()
                .map(|item| item.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["mute", "remove"]
        );
    }

    #[test]
    fn ai_rule_actions_require_their_independent_permissions() {
        let intents = ["recall", "mute", "remove", "blacklist", "notify"]
            .into_iter()
            .map(|kind| moderation::ActionIntent {
                action: action(kind),
                contributors: Vec::new(),
            })
            .collect::<Vec<_>>();
        let allowed = filter_ai_rule_intents(intents, true, false, false, true);
        assert_eq!(
            allowed
                .iter()
                .map(|intent| intent.action.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["recall", "notify"]
        );
    }

    #[test]
    fn ai_rules_only_classify_non_empty_text_messages() {
        assert!(message_supports_ai_rules("text", "正常文本"));
        assert!(!message_supports_ai_rules("text", ""));
        assert!(!message_supports_ai_rules("image", "[消息解码失败]"));
    }

    #[test]
    fn member_card_name_is_preferred_for_message_and_audit_display() {
        assert_eq!(
            preferred_member_display_name("我知", "DH群员0001", "我不知道啊").as_deref(),
            Some("我知")
        );
    }

    #[test]
    fn only_external_member_card_changes_increment_rename_violations() {
        assert!(is_external_member_card_update(
            true,
            true,
            "原名",
            "外部改名",
            false
        ));
        assert!(!is_external_member_card_update(
            true, true, "原名", "DH改名", true
        ));
        assert!(!is_external_member_card_update(
            true, true, "原名", "", false
        ));
        assert!(!is_external_member_card_update(
            false,
            true,
            "",
            "首次发现",
            false
        ));
        assert!(!is_external_member_card_update(
            true, true, "相同", " 相同 ", false
        ));
    }

    #[test]
    fn unlocked_members_never_accumulate_rename_violations() {
        // 没有锁定群名片的成员自行改名不属于违规，避免 rename_count 规则误伤。
        assert!(!is_external_member_card_update(
            true, false, "原名", "自己改的名", false
        ));
    }

    #[test]
    fn connection_retry_is_bounded_and_fast() {
        let mut delay = Duration::from_millis(250);
        let mut observed = Vec::new();
        for _ in 0..5 {
            observed.push(delay.as_millis());
            delay = next_connection_retry(delay);
        }
        assert_eq!(observed, vec![250, 500, 1_000, 2_000, 2_000]);
    }

    #[test]
    fn http_business_1001_is_terminal_and_never_retried() {
        let mut metadata = crate::error::GatewayErrorMetadata::new(
            "/v1/group/message-rollback",
            crate::error::GatewayErrorLayer::Business,
        );
        metadata.kind = crate::error::GatewayErrorKind::Business;
        metadata.business_code = Some(1001);
        let error = AppError::new("gateway_business", "禁止调用此接口")
            .with_gateway(metadata)
            .retryable();
        assert_eq!(
            classify_effect_error(&error),
            EffectDispatchStatus::TerminalFailed
        );
    }

    #[test]
    fn connection_loop_never_schedules_member_roster_refreshes() {
        let source = include_str!("runtime.rs");
        let connection_loop = source
            .split_once("    async fn connection_loop")
            .and_then(|(_, remainder)| remainder.split_once("    async fn handle_gateway_event"))
            .map(|(body, _)| body)
            .expect("connection loop source must remain inspectable");

        assert!(!connection_loop.contains("list_members("));
        assert!(!connection_loop.contains("last_roster_sync"));
        assert!(!connection_loop.contains("sync_members("));
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

    #[test]
    fn gateway_ack_stops_before_unpersisted_member_event() {
        let records = (1..=3)
            .map(|sequence| GatewayRecord {
                session: "session".into(),
                sequence,
                kind: if sequence == 2 {
                    GatewayRecordKind::TeamMemberJoined
                } else {
                    GatewayRecordKind::Message
                },
                source: "fixture".into(),
                payload: serde_json::json!({"idServer":format!("m-{sequence}")}),
            })
            .collect::<Vec<_>>();
        let ackable = [
            gateway_record_id(&records[0]),
            gateway_record_id(&records[2]),
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(contiguous_ack_sequence(&records, &ackable), 1);
        let ackable = records
            .iter()
            .map(gateway_record_id)
            .collect::<HashSet<_>>();
        assert_eq!(contiguous_ack_sequence(&records, &ackable), 3);
    }

    #[test]
    fn real_wang_mention_metadata_matches_logged_in_account() {
        let mut message = Message {
            id: 1,
            account_id: "1667937946".into(),
            group_id: 1143980,
            server_message_id: "message".into(),
            sequence: 1,
            user_id: 31846829,
            sender_name: "成员".into(),
            kind: "text".into(),
            text: "@迪奥 你好".into(),
            sent_at: Utc::now(),
            received_at: Utc::now(),
            processed_at: None,
            acknowledged_at: None,
            processing_state: "pending".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: serde_json::json!({
                "AiTeInfo": [{"end": 4, "nick": "迪奥", "uid": 1667937946_i64}],
                "content": {"data": "@迪奥 你好", "maskWords": []}
            })
            .to_string(),
            source_kind: Some("onmsg".into()),
            flow: Some("in".into()),
        };
        assert!(message_explicitly_mentions(&message, "1667937946", 9798577));
        assert!(!message_explicitly_mentions(&message, "other-account", 123));
        message.mentions_json = serde_json::json!({
            "AiTeInfo": [{"end": 4, "nick": "迪奥", "uid": 9798577_i64}],
            "content": {"data": "@迪奥 你好", "maskWords": []}
        })
        .to_string();
        assert!(message_explicitly_mentions(&message, "1667937946", 9798577));
    }

    #[test]
    fn decoded_custom_text_is_classified_as_text() {
        assert_eq!(normalized_message_kind(Some("custom"), 99, true), "text");
        assert_eq!(normalized_message_kind(Some("custom"), 99, false), "other");
        assert_eq!(normalized_message_kind(None, 0, false), "text");
    }

    #[tokio::test]
    async fn raw_nim_sender_is_resolved_through_the_identity_alias() {
        let (_directory, paths) = test_paths();
        let database = Database::open(&paths).unwrap();
        let now = Utc::now();
        database
            .upsert_account(&Account {
                id: FIXTURE_ACCOUNT.into(),
                display_name: "测试账号".into(),
                role: "admin".into(),
                discovered_at: now,
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_group(&Group {
                account_id: FIXTURE_ACCOUNT.into(),
                group_id: FIXTURE_GROUP,
                name: "测试群".into(),
                owner_user_id: 10001,
                enabled: true,
                ai_enabled: false,
                moderation_enabled: true,
                machine_rules_enabled: true,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .unwrap();
        database
            .upsert_member(&Member {
                account_id: FIXTURE_ACCOUNT.into(),
                group_id: FIXTURE_GROUP,
                user_id: 31846829,
                nim_id: "1938341073".into(),
                nickname: "其他成员".into(),
                card_name: "其他成员".into(),
                original_card_name: String::new(),
                managed_card_name: String::new(),
                card_suffix: String::new(),
                role: "member".into(),
                account_state: String::new(),
                blacklisted: false,
                present: true,
                join_source: "baseline".into(),
                prompt_read: true,
                locked_card_name: String::new(),
                violation_count: 0,
                discovered_at: now,
                joined_at: None,
                last_seen_at: now,
                updated_at: now,
            })
            .unwrap();
        let database = DatabaseExecutor::start(database).unwrap();
        let runtime = BackendRuntime::new(
            database.clone(),
            Arc::new(FixtureGateway::new_default()),
            SecretStore::new(&paths.secrets),
            Arc::new(ShutdownSignal::default()),
            Logger::new(&paths.logs),
        );
        let raw = serde_json::json!({
            "seq": 1,
            "groupId": FIXTURE_GROUP,
            "idServer": "raw-nim-message",
            "scene": "team",
            "from": "1938341073",
            "flow": "in",
            "type": "text",
            "content": "测试消息",
            "decoded": null
        });
        let job = runtime
            .normalize_message(FIXTURE_ACCOUNT, 10001, &raw)
            .await
            .unwrap();
        assert_eq!(job.message.user_id, 31846829);
        database.shutdown().await.unwrap();
    }

    #[test]
    fn real_wang_mention_uses_readable_text_instead_of_encrypted_content() {
        let raw = serde_json::json!({
            "type": "custom",
            "content": "ENCRYPTED_PAYLOAD",
            "decoded": {
                "mentions": {
                    "AiTeInfo": [{"nick": "迪奥", "uid": 1667937946_i64}],
                    "content": {"data": "@迪奥 你好", "maskWords": []}
                }
            }
        });
        assert_eq!(normalized_message_text(&raw["decoded"], &raw), "@迪奥 你好");

        let legacy_raw = serde_json::json!({
            "type": "custom",
            "content": "ENCRYPTED_PAYLOAD",
            "mentions": {
                "AiTeInfo": [{"nick": "迪奥", "uid": 1667937946_i64}],
                "content": {"data": "@迪奥 你好", "maskWords": []}
            }
        });
        assert_eq!(
            normalized_message_text(&Value::Null, &legacy_raw),
            "@迪奥 你好"
        );
    }

    #[test]
    fn gateway_receipts_are_redacted_before_archiving() {
        let secret = ["sk", "-", "abcdefghijklmnopqrstuvwxyz"].concat();
        let receipt = redact_gateway_receipt(GatewayReceipt {
            route: format!("/route?token={secret}"),
            status: "failed".into(),
            transport_code: Some(500),
            transport_errno: Some(1),
            business_code: Some(500),
            business_errno: Some(2),
            business_message: format!("Authorization: Bearer TOKEN apiKey={secret}"),
            request_id: format!("request-{secret}"),
            message_id: format!("message-{secret}"),
            session: format!("session-{secret}"),
            acknowledged_through: 0,
            acknowledged: 0,
            remaining: 0,
            dropped: 0,
            verification: None,
        });
        let archived = serde_json::to_string(&receipt).unwrap();
        assert!(!archived.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!archived.contains("Bearer TOKEN"));
        assert!(archived.contains("***"));
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
            Arc::new(ShutdownSignal::default()),
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
        let permit = runtime.gateway.automatic_write_permit().await.unwrap();
        runtime
            .dispatch_effect_permitted(None, items[0].clone(), &permit)
            .await;
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

    #[tokio::test]
    async fn automatic_machine_rule_recalls_another_members_message_once() {
        let (_directory, paths) = test_paths();
        let database = Database::open(&paths).unwrap();
        let database = DatabaseExecutor::start(database).unwrap();
        let fixture = Arc::new(FixtureGateway::new_default());
        fixture
            .emit_message(
                FIXTURE_GROUP,
                10006,
                "请提供验证码",
                Some(88),
                Some("machine-rule-message".into()),
            )
            .await
            .unwrap();
        let runtime = BackendRuntime::new(
            database.clone(),
            fixture.clone(),
            SecretStore::new(&paths.secrets),
            Arc::new(ShutdownSignal::default()),
            Logger::new(&paths.logs),
        );
        let message = Message {
            id: 88,
            account_id: FIXTURE_ACCOUNT.into(),
            group_id: FIXTURE_GROUP,
            server_message_id: "machine-rule-message".into(),
            sequence: 88,
            user_id: 10006,
            sender_name: "普通成员".into(),
            kind: "text".into(),
            text: "请提供验证码".into(),
            sent_at: Utc::now(),
            received_at: Utc::now(),
            processed_at: None,
            acknowledged_at: None,
            processing_state: "processing".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: Some("onmsg".into()),
            flow: Some("in".into()),
        };
        let intent = moderation::ActionIntent {
            action: action("recall"),
            contributors: vec![moderation::ActionContributor {
                rule_id: 7,
                rule_name: "验证码风险".into(),
                priority: 3,
            }],
        };
        let ai_intent = moderation::ActionIntent {
            action: action("recall"),
            contributors: vec![moderation::ActionContributor {
                rule_id: 13,
                rule_name: "AI 诈骗识别".into(),
                priority: 3,
            }],
        };
        for candidate in [&intent, &ai_intent] {
            runtime
                .execute_rule_actions(
                    None,
                    FIXTURE_ACCOUNT,
                    10001,
                    &message,
                    std::slice::from_ref(candidate),
                    true,
                )
                .await;
        }
        let items = database
            .claim_effect_outbox(Some(FIXTURE_ACCOUNT.into()), 10)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        let permit = runtime.gateway.automatic_write_permit().await.unwrap();
        runtime
            .dispatch_effect_permitted(None, items[0].clone(), &permit)
            .await;
        let snapshot = fixture.snapshot().await;
        let recalls = snapshot
            .actions
            .iter()
            .filter(|item| item.kind == "recall")
            .collect::<Vec<_>>();
        assert_eq!(recalls.len(), 1);
        assert_eq!(recalls[0].group_id, FIXTURE_GROUP);
        assert_eq!(recalls[0].user_id, 10006);
        assert_eq!(recalls[0].text, "machine-rule-message");
        assert_eq!(
            snapshot.recalled_messages,
            vec!["machine-rule-message".to_string()]
        );
        database.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn roster_reconciliation_enqueues_offline_member_card_and_welcome_job() {
        let (_directory, paths) = test_paths();
        let database = Database::open(&paths).unwrap();
        let database = DatabaseExecutor::start(database).unwrap();
        let fixture = Arc::new(FixtureGateway::new_default());
        let runtime = BackendRuntime::new(
            database.clone(),
            fixture.clone(),
            SecretStore::new(&paths.secrets),
            Arc::new(ShutdownSignal::default()),
            Logger::new(&paths.logs),
        );
        let now = Utc::now();
        database
            .upsert_account(Account {
                id: FIXTURE_ACCOUNT.into(),
                display_name: "Fixture 管理员".into(),
                role: "owner".into(),
                discovered_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let group = fixture
            .list_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|group| group.group_id == FIXTURE_GROUP)
            .unwrap();
        database.upsert_group(group).await.unwrap();
        database
            .set_group_features(
                FIXTURE_ACCOUNT.into(),
                FIXTURE_GROUP,
                true,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        database
            .set_setting(
                format!("card.auto.{FIXTURE_ACCOUNT}.{FIXTURE_GROUP}"),
                "true".into(),
                false,
            )
            .await
            .unwrap();

        runtime.sync_members(FIXTURE_ACCOUNT).await;
        assert!(database
            .list_card_jobs(FIXTURE_ACCOUNT.into(), FIXTURE_GROUP, 20)
            .await
            .unwrap()
            .is_empty());

        fixture
            .resize_group_members(FIXTURE_GROUP, 17)
            .await
            .unwrap();
        runtime.sync_members(FIXTURE_ACCOUNT).await;

        let jobs = database
            .list_card_jobs(FIXTURE_ACCOUNT.into(), FIXTURE_GROUP, 20)
            .await
            .unwrap();
        // The new member is queued with a welcome, while the existing `1`
        // and `.` fixture cards are independently repaired as malformed
        // group-card values.
        assert_eq!(jobs.len(), 3);
        let joined = jobs.iter().find(|job| job.user_id == 10017).unwrap();
        assert_eq!(joined.state, "queued");
        assert!(joined.welcome_pending);
        assert!(jobs.iter().any(|job| job.user_id == 10004));
        assert!(jobs.iter().any(|job| job.user_id == 10005));
        database.shutdown().await.unwrap();
    }

    #[cfg(feature = "fixture")]
    #[tokio::test]
    async fn calibration_waits_for_claimed_effect_and_does_not_consume_next_attempt() {
        let (_directory, paths) = test_paths();
        let database = Database::open(&paths).unwrap();
        let database = DatabaseExecutor::start(database).unwrap();
        let gateway = Arc::new(CdpGateway::new(
            CdpClient::new("http://127.0.0.1:9222").unwrap(),
        ));
        let runtime = BackendRuntime::new(
            database.clone(),
            gateway.clone(),
            SecretStore::new(&paths.secrets),
            Arc::new(ShutdownSignal::default()),
            Logger::new(&paths.logs),
        );
        let fixture = FixtureGateway::new_default();
        let now = Utc::now();
        database
            .upsert_account(Account {
                id: FIXTURE_ACCOUNT.into(),
                display_name: "Fixture 管理员".into(),
                role: "owner".into(),
                discovered_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let group = fixture
            .list_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|group| group.group_id == FIXTURE_GROUP)
            .unwrap();
        database.upsert_group(group).await.unwrap();
        let members = fixture.list_members(FIXTURE_GROUP).await.unwrap().members;
        for member in members
            .into_iter()
            .filter(|member| matches!(member.user_id, 10006 | 10007))
        {
            database.upsert_member(member).await.unwrap();
        }
        for (user_id, dedupe_key) in [(10006, "first"), (10007, "second")] {
            database
                .enqueue_effect(EffectOutboxRequest {
                    account_id: FIXTURE_ACCOUNT.into(),
                    group_id: FIXTURE_GROUP,
                    effect_type: "blacklist".into(),
                    payload_json: serde_json::json!({"userId":user_id}).to_string(),
                    dedupe_key: dedupe_key.into(),
                })
                .await
                .unwrap();
        }
        let permit = runtime
            .gateway
            .automatic_write_permit()
            .await
            .unwrap();
        let first = runtime
            .database
            .claim_effect_outbox(Some(FIXTURE_ACCOUNT.into()), 1)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let starting_gateway = gateway.clone();
        let starting = tokio::spawn(async move {
            starting_gateway
                .begin_developer_calibration(
                    CalibrationMetadata {
                        app_file_version: "2.7.8".into(),
                        main_script_sha256: "a".repeat(64),
                        page_title: "旺商聊".into(),
                        page_url: "file:///index.html".into(),
                    },
                    vec!["rename".into()],
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(!starting.is_finished());

        runtime
            .dispatch_effect_permitted(None, first, &permit)
            .await;
        drop(permit);
        assert!(starting.await.unwrap().unwrap().active);
        assert_eq!(
            runtime
                .gateway
                .automatic_write_permit()
                .await
                .unwrap_err()
                .code,
            "calibration_active"
        );
        let rows = database
            .execute(|database| {
                database
                    .with_connection(|connection| {
                        let mut statement = connection.prepare(
                            "SELECT dedupe_key,state,attempts FROM effect_outbox ORDER BY id",
                        )?;
                        let rows = statement
                            .query_map([], |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, i64>(2)?,
                                ))
                            })?
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(rows)
                    })
                    .map_err(crate::error::InternalError::from)
                    .map_err(AppError::from)
            })
            .await
            .unwrap();
        assert_eq!(rows[0], ("first".into(), "succeeded".into(), 1));
        assert_eq!(rows[1], ("second".into(), "queued".into(), 0));
        gateway.cancel_developer_calibration().unwrap();
        database.shutdown().await.unwrap();
    }
}
