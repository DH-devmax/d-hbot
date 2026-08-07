use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::defaults;
use crate::error::{AppError, AppResult};
use crate::models::{AiDecision, AiTask, Message, RuleAction};

pub const PERSONA: &str = include_str!("../../../AGENTS.md");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MentionMetadata {
    pub alias: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiContextMessage {
    pub user_id: i64,
    pub name: String,
    pub text: String,
    pub time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiKnowledgeChunk {
    pub base: String,
    pub title: String,
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRequest {
    pub version: &'static str,
    pub event_id: String,
    #[serde(skip_serializing)]
    pub persona: &'static str,
    pub group_id: i64,
    pub group_name: String,
    pub member_id: i64,
    pub member_name: String,
    pub member_role: String,
    pub message_id: String,
    pub message: String,
    pub recent_context: Vec<AiContextMessage>,
    pub knowledge: Vec<AiKnowledgeChunk>,
}

impl AiRequest {
    pub fn testing(message: impl Into<String>, recent_context: Vec<AiContextMessage>) -> Self {
        Self::testing_with_knowledge(message, recent_context, false)
    }

    pub fn testing_with_knowledge(
        message: impl Into<String>,
        recent_context: Vec<AiContextMessage>,
        include_built_in_knowledge: bool,
    ) -> Self {
        Self {
            version: "1",
            event_id: Uuid::new_v4().to_string(),
            persona: PERSONA,
            group_id: 1,
            group_name: "本地 AI 测试".into(),
            member_id: 1,
            member_name: "测试用户".into(),
            member_role: "admin".into(),
            message_id: Uuid::new_v4().to_string(),
            message: message.into(),
            recent_context,
            knowledge: if include_built_in_knowledge {
                default_knowledge_chunks()
            } else {
                Vec::new()
            },
        }
    }
}

pub fn default_knowledge_chunks() -> Vec<AiKnowledgeChunk> {
    defaults::DEFAULT_KNOWLEDGE_DOCUMENTS
        .iter()
        .map(|document| AiKnowledgeChunk {
            base: defaults::DEFAULT_KNOWLEDGE_BASE_NAME.into(),
            title: document.title.into(),
            source: "built-in".into(),
            text: document.content.into(),
        })
        .collect()
}

pub fn mention_metadata(text: &str) -> Option<MentionMetadata> {
    let matcher = regex::Regex::new(r"(?i)@\s*dh").expect("static mention regex");
    let found = matcher.find_iter(text).find_map(|matched| {
        let boundary = text[matched.end()..].chars().next();
        if boundary.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_') {
            return None;
        }
        Some(MentionMetadata {
            alias: text[matched.start()..matched.end()].to_string(),
            start: matched.start(),
            end: matched.end(),
        })
    });
    found
}

pub fn message_without_mention(text: &str) -> String {
    let Some(metadata) = mention_metadata(text) else {
        return text.trim().to_string();
    };
    let mut message = String::with_capacity(text.len() - (metadata.end - metadata.start));
    message.push_str(&text[..metadata.start]);
    message.push_str(&text[metadata.end..]);
    message.trim().to_string()
}

/// Builds a chronological, bounded context from messages in any input order.
pub fn build_recent_context(messages: &[Message], limit: usize) -> Vec<AiContextMessage> {
    if limit == 0 {
        return Vec::new();
    }
    let mut sorted = messages.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        left.sent_at
            .cmp(&right.sent_at)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.id.cmp(&right.id))
    });
    let start = sorted.len().saturating_sub(limit);
    sorted[start..]
        .iter()
        .map(|message| AiContextMessage {
            user_id: message.user_id,
            name: message.sender_name.clone(),
            text: message.text.clone(),
            time: message.sent_at.timestamp_millis(),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct AiConfig {
    pub base_url: String,
    pub webhook_url: String,
    /// `chat_completions` is the OpenAI-compatible default. `responses` uses
    /// the OpenAI Responses envelope while keeping the same DH decision schema.
    pub api_backend: String,
    pub model: String,
    /// Responses reasoning depth: low, medium, high, or xhigh.
    pub reasoning_effort: String,
    pub api_key: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTestResult {
    pub decision: AiDecision,
    pub model: String,
    pub elapsed_ms: u128,
    pub knowledge_source: String,
}

#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn decide(&self, request: &AiRequest) -> AppResult<AiDecision>;

    fn last_attempt_count(&self) -> usize {
        1
    }
}

pub struct ConfiguredProvider {
    config: AiConfig,
    client: reqwest::Client,
    responses_route_unavailable: AtomicBool,
}

pub const AI_PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);
pub const AI_TOTAL_BUDGET: Duration = Duration::from_secs(20);
const RESPONSES_ROUTE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

fn ai_transport_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "AI 请求超时，请检查网络或服务状态"
    } else if error.is_connect() {
        "AI 服务连接失败，请检查网络、代理或服务地址"
    } else {
        "AI 网络请求失败，请检查网络或服务地址"
    }
}

impl ConfiguredProvider {
    pub fn new(mut config: AiConfig) -> AppResult<Self> {
        if config.model.trim().is_empty() {
            config.model = "deepseek-v4-pro".into();
        }
        config.api_backend = normalize_api_backend(&config.api_backend)?;
        config.reasoning_effort = normalize_reasoning_effort(&config.reasoning_effort)?;
        if config.timeout.is_zero() || config.timeout > AI_PROVIDER_TIMEOUT {
            config.timeout = AI_PROVIDER_TIMEOUT;
        }
        validate_remote_url(if config.webhook_url.trim().is_empty() {
            &config.base_url
        } else {
            &config.webhook_url
        })?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(config.timeout)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|_| AppError::new("ai_client", "AI 网络客户端初始化失败"))?;
        Ok(Self {
            config,
            client,
            responses_route_unavailable: AtomicBool::new(false),
        })
    }

    pub async fn test(&self, request: &AiRequest) -> AppResult<AiTestResult> {
        let started = Instant::now();
        let decision = self.decide(request).await?;
        Ok(AiTestResult {
            decision,
            model: self.config.model.clone(),
            elapsed_ms: started.elapsed().as_millis(),
            knowledge_source: if request.knowledge.is_empty() {
                "空上下文".into()
            } else {
                "DH 默认群规与 FAQ".into()
            },
        })
    }

    async fn decide_openai(&self, request: &AiRequest) -> AppResult<AiDecision> {
        if self.config.base_url.trim().is_empty() {
            return Err(AppError::new("ai_not_configured", "请先填写 AI Base URL"));
        }
        let url = completion_url(&self.config.base_url)?;
        let request_json = serde_json::to_string(&bounded_request(request))
            .map_err(|error| AppError::new("ai_request", error.to_string()))?;
        let body = json!({
            "model": self.config.model,
            "temperature": 0.3,
            "max_tokens": 512,
            "reasoning_effort": self.config.reasoning_effort,
            "response_format": {"type":"json_object"},
            "messages": [
                {"role":"system","content":format!("{}\n\n只返回一个 JSON 对象，字段只能是 reply、actions、tasks、confidence、reason。群聊回复不得泄露接口地址、认证信息、上游字段和原始响应结构。", request.persona)},
                {"role":"user","content":request_json}
            ]
        });
        let mut outbound = self.client.post(url).json(&body);
        if !self.config.api_key.trim().is_empty() {
            outbound = outbound.bearer_auth(self.config.api_key.trim());
        }
        let response = outbound
            .send()
            .await
            .map_err(|error| AppError::new("ai_request", ai_transport_error(&error)).retryable())?;
        let status = response.status();
        let bytes = read_capped_ai_body(response).await?;
        if !status.is_success() {
            return Err(AppError::new("ai_http", format!("AI 请求返回 HTTP {status}")).retryable());
        }
        let envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::new("ai_response", format!("AI 响应格式错误：{error}")))?;
        let content = envelope
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::new("ai_response", "AI 响应中没有可用结果"))?;
        decode_decision(content)
    }

    async fn decide_responses(&self, request: &AiRequest) -> AppResult<AiDecision> {
        if self.config.base_url.trim().is_empty() {
            return Err(AppError::new("ai_not_configured", "请先填写 AI Base URL"));
        }
        let url = responses_url(&self.config.base_url)?;
        let request_json = serde_json::to_string(&bounded_request(request))
            .map_err(|error| AppError::new("ai_request", error.to_string()))?;
        let body = json!({
            "model": self.config.model,
            "max_output_tokens": 1024,
            "reasoning": {"effort": self.config.reasoning_effort},
            "instructions": response_instruction(request.persona),
            "input": request_json,
            "text": {"format": {"type": "json_object"}}
        });
        let mut outbound = self.client.post(url).json(&body);
        if !self.config.api_key.trim().is_empty() {
            outbound = outbound.bearer_auth(self.config.api_key.trim());
        }
        let response = outbound
            .send()
            .await
            .map_err(|error| AppError::new("ai_request", ai_transport_error(&error)).retryable())?;
        let status = response.status();
        let bytes = read_capped_ai_body(response).await?;
        if !status.is_success() {
            return Err(AppError::new(
                "ai_http",
                format!("AI 请求返回 HTTP {status}{}", response_error_suffix(&bytes)),
            )
            .retryable());
        }
        let envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::new("ai_response", format!("AI 响应格式错误：{error}")))?;
        let content = response_output_text(&envelope)
            .ok_or_else(|| AppError::new("ai_response", "AI Responses 响应中没有可用结果"))?;
        decode_decision(content)
    }

    async fn decide_webhook(&self, request: &AiRequest) -> AppResult<AiDecision> {
        let response = self
            .client
            .post(self.config.webhook_url.trim())
            .json(&bounded_request(request))
            .send()
            .await
            .map_err(|error| {
                AppError::new("webhook_request", ai_transport_error(&error)).retryable()
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                AppError::new("webhook_http", format!("Webhook 返回 HTTP {status}")).retryable(),
            );
        }
        // Webhook 地址由用户配置，指向远端；响应体必须边读边限长。
        let bytes = crate::http_body::read_capped_body(response, AI_RESPONSE_LIMIT)
            .await
            .map_err(|_| AppError::new("webhook_response", "Webhook 响应读取失败"))?;
        let wire: WireDecision = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::new("webhook_response", "Webhook 响应格式错误"))?;
        validate_decision(wire.into())
    }
}

/// AI 提供方响应体上限。
///
/// AI 决策本身只有几 KB，但这里沿用共享默认值：要关掉的缺陷是"无上限"，
/// 不是"上限太大"。收紧到一个我无法验证的数值，反而可能挡掉正常响应。
const AI_RESPONSE_LIMIT: usize = crate::http_body::DEFAULT_RESPONSE_LIMIT;

/// 读取 AI 提供方响应体并限长，把失败映射成本模块的错误码。
///
/// `base_url` 由用户配置、指向远端，所以不能直接 `bytes().await`。
/// 文案与错误码保持与限长前一致，避免影响依赖错误码的上层逻辑。
async fn read_capped_ai_body(response: reqwest::Response) -> AppResult<Vec<u8>> {
    crate::http_body::read_capped_body(response, AI_RESPONSE_LIMIT)
        .await
        .map_err(|_| AppError::new("ai_response", "AI 响应读取失败，请稍后重试"))
}

fn response_error_suffix(bytes: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return String::new();
    };
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if message.is_empty() {
        String::new()
    } else {
        let sanitized = message
            .replace("Authorization", "认证信息")
            .replace("api_key", "密钥")
            .chars()
            .take(240)
            .collect::<String>();
        format!("：{sanitized}")
    }
}

const MAX_RECENT_MESSAGES: usize = 8;
const MAX_RECENT_CHARS: usize = 2_400;
const MAX_KNOWLEDGE_CHUNKS: usize = 3;
const MAX_KNOWLEDGE_CHARS: usize = 3_600;

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn bounded_request(request: &AiRequest) -> AiRequest {
    let mut recent_context = request
        .recent_context
        .iter()
        .rev()
        .take(MAX_RECENT_MESSAGES)
        .cloned()
        .collect::<Vec<_>>();
    recent_context.reverse();
    let mut remaining = MAX_RECENT_CHARS;
    for item in recent_context.iter_mut().rev() {
        item.text = truncate_chars(&item.text, remaining);
        remaining = remaining.saturating_sub(item.text.chars().count());
    }
    recent_context.retain(|item| !item.text.is_empty());

    let mut knowledge = request
        .knowledge
        .iter()
        .take(MAX_KNOWLEDGE_CHUNKS)
        .cloned()
        .collect::<Vec<_>>();
    let mut remaining = MAX_KNOWLEDGE_CHARS;
    for item in &mut knowledge {
        item.text = truncate_chars(&item.text, remaining);
        remaining = remaining.saturating_sub(item.text.chars().count());
    }
    knowledge.retain(|item| !item.text.is_empty());

    AiRequest {
        version: request.version,
        event_id: request.event_id.clone(),
        persona: request.persona,
        group_id: request.group_id,
        group_name: request.group_name.clone(),
        member_id: request.member_id,
        member_name: request.member_name.clone(),
        member_role: request.member_role.clone(),
        message_id: request.message_id.clone(),
        message: request.message.clone(),
        recent_context,
        knowledge,
    }
}

fn config_fingerprint(config: &AiConfig) -> String {
    let mut hasher = Sha256::new();
    for value in [
        config.base_url.trim(),
        config.webhook_url.trim(),
        config.api_backend.trim(),
        config.model.trim(),
        config.reasoning_effort.trim(),
        config.api_key.trim(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Clone, Default)]
pub struct AiProviderPool {
    inner: Arc<Mutex<ProviderPoolState>>,
}

#[derive(Default)]
struct ProviderPoolState {
    providers: HashMap<String, Arc<ConfiguredProvider>>,
    health: HashMap<String, ProviderHealth>,
}

#[derive(Default)]
struct ProviderHealth {
    failures: u32,
    cooldown_until: Option<Instant>,
}

impl AiProviderPool {
    pub fn provider(&self, config: AiConfig) -> AppResult<Arc<ConfiguredProvider>> {
        let fingerprint = config_fingerprint(&config);
        let mut state = self
            .inner
            .lock()
            .map_err(|_| AppError::new("ai_pool", "AI 连接池状态异常"))?;
        if let Some(provider) = state.providers.get(&fingerprint) {
            return Ok(provider.clone());
        }
        let provider = Arc::new(ConfiguredProvider::new(config)?);
        state.providers.insert(fingerprint, provider.clone());
        Ok(provider)
    }

    pub fn chain(&self, configs: Vec<AiConfig>) -> AppResult<Arc<dyn AiProvider>> {
        if configs.is_empty() {
            return Err(AppError::new("ai_not_configured", "请先配置 AI 连接"));
        }
        let mut providers = Vec::with_capacity(configs.len());
        let mut last_error = None;
        for config in configs {
            let fingerprint = config_fingerprint(&config);
            match self.provider(config) {
                Ok(provider) => providers.push((fingerprint, provider)),
                Err(error) => last_error = Some(error),
            }
        }
        if providers.is_empty() {
            return Err(last_error
                .unwrap_or_else(|| AppError::new("ai_not_configured", "请先配置 AI 连接")));
        }
        Ok(Arc::new(PooledProviderChain {
            pool: self.clone(),
            providers,
            last_attempts: AtomicUsize::new(0),
        }))
    }

    fn available(&self, fingerprint: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .health
                    .get(fingerprint)
                    .and_then(|health| health.cooldown_until)
            })
            .is_none_or(|until| until <= Instant::now())
    }

    fn succeeded(&self, fingerprint: &str) {
        if let Ok(mut state) = self.inner.lock() {
            state.health.remove(fingerprint);
        }
    }

    fn failed(&self, fingerprint: &str, error: &AppError) {
        if let Ok(mut state) = self.inner.lock() {
            let health = state.health.entry(fingerprint.into()).or_default();
            health.failures = health.failures.saturating_add(1);
            let auth_error = error.message.contains("401") || error.message.contains("403");
            let seconds = if auth_error {
                30 * 60
            } else {
                (30_u64.saturating_mul(1_u64 << health.failures.saturating_sub(1).min(4))).min(600)
            };
            health.cooldown_until = Some(Instant::now() + Duration::from_secs(seconds));
        }
    }
}

struct PooledProviderChain {
    pool: AiProviderPool,
    providers: Vec<(String, Arc<ConfiguredProvider>)>,
    last_attempts: AtomicUsize,
}

#[async_trait]
impl AiProvider for PooledProviderChain {
    async fn decide(&self, request: &AiRequest) -> AppResult<AiDecision> {
        self.last_attempts.store(0, Ordering::Relaxed);
        let started = Instant::now();
        let mut attempted = 0;
        let mut last_error = None;
        for (fingerprint, provider) in &self.providers {
            if attempted >= 2 || started.elapsed() >= AI_TOTAL_BUDGET {
                break;
            }
            if !self.pool.available(fingerprint) && self.providers.len() > 1 {
                continue;
            }
            attempted += 1;
            self.last_attempts.store(attempted, Ordering::Relaxed);
            let remaining = AI_TOTAL_BUDGET.saturating_sub(started.elapsed());
            let budget = remaining.min(AI_PROVIDER_TIMEOUT);
            match tokio::time::timeout(budget, provider.decide(request)).await {
                Ok(Ok(decision)) => {
                    self.pool.succeeded(fingerprint);
                    return Ok(decision);
                }
                Ok(Err(error)) => {
                    self.pool.failed(fingerprint, &error);
                    last_error = Some(error);
                }
                Err(_) => {
                    let error = AppError::new("ai_request", "AI 请求超时，请检查网络或服务状态")
                        .retryable();
                    self.pool.failed(fingerprint, &error);
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            AppError::new("ai_provider_cooldown", "AI 连接正在冷却，请稍后重试").retryable()
        }))
    }

    fn last_attempt_count(&self) -> usize {
        self.last_attempts.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub struct AiReplyCache {
    inner: Arc<Mutex<HashMap<String, ReplyCacheState>>>,
    capacity: usize,
    ttl: Duration,
}

#[derive(Clone)]
pub struct AiKnowledgeCache {
    inner: Arc<Mutex<KnowledgeCacheEntries>>,
    ttl: Duration,
}

type KnowledgeCacheEntries = HashMap<String, (Instant, Vec<AiKnowledgeChunk>)>;

impl Default for AiKnowledgeCache {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(60),
        }
    }
}

impl AiKnowledgeCache {
    pub fn get(&self, key: &str) -> Option<Vec<AiKnowledgeChunk>> {
        let mut state = self.inner.lock().ok()?;
        let (created_at, chunks) = state.get(key)?.clone();
        if created_at.elapsed() > self.ttl {
            state.remove(key);
            return None;
        }
        Some(chunks)
    }

    pub fn insert(&self, key: String, chunks: Vec<AiKnowledgeChunk>) {
        if let Ok(mut state) = self.inner.lock() {
            state.retain(|_, (created_at, _)| created_at.elapsed() <= self.ttl);
            if state.len() >= 512 {
                if let Some(oldest) = state
                    .iter()
                    .min_by_key(|(_, (created_at, _))| *created_at)
                    .map(|(key, _)| key.clone())
                {
                    state.remove(&oldest);
                }
            }
            state.insert(key, (Instant::now(), chunks));
        }
    }
}

pub fn knowledge_cache_key(
    account_id: &str,
    group_id: i64,
    question: &str,
    knowledge_revision: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        account_id,
        &group_id.to_string(),
        question,
        knowledge_revision,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Default)]
struct ReplyCacheState {
    entries: HashMap<String, (Instant, AiDecision)>,
    order: VecDeque<String>,
}

impl Default for AiReplyCache {
    fn default() -> Self {
        Self::new(512, Duration::from_secs(10 * 60))
    }
}

impl AiReplyCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            capacity: capacity.max(1),
            ttl,
        }
    }

    pub fn get_scoped(&self, namespace: &str, key: &str) -> Option<AiDecision> {
        let mut namespaces = self.inner.lock().ok()?;
        let state = namespaces.get_mut(namespace)?;
        let (created_at, decision) = state.entries.get(key)?.clone();
        if created_at.elapsed() > self.ttl {
            state.entries.remove(key);
            state.order.retain(|item| item != key);
            return None;
        }
        state.order.retain(|item| item != key);
        state.order.push_back(key.into());
        Some(decision)
    }

    pub fn insert_scoped(&self, namespace: &str, key: String, decision: AiDecision) -> bool {
        if decision.reply.trim().is_empty()
            || !decision.actions.is_empty()
            || !decision.tasks.is_empty()
        {
            return false;
        }
        if let Ok(mut namespaces) = self.inner.lock() {
            let state = namespaces.entry(namespace.into()).or_default();
            state.order.retain(|item| item != &key);
            state.order.push_back(key.clone());
            state.entries.insert(key, (Instant::now(), decision));
            while state.entries.len() > self.capacity {
                if let Some(oldest) = state.order.pop_front() {
                    state.entries.remove(&oldest);
                } else {
                    break;
                }
            }
            true
        } else {
            false
        }
    }

    pub fn remove_scoped(&self, namespace: &str, key: &str) {
        if let Ok(mut namespaces) = self.inner.lock() {
            if let Some(state) = namespaces.get_mut(namespace) {
                state.entries.remove(key);
                state.order.retain(|item| item != key);
            }
        }
    }
}

pub fn answer_cache_key(
    account_id: &str,
    group_id: i64,
    question: &str,
    knowledge_revision: &str,
    provider_revision: &str,
) -> String {
    let normalized = question
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_ascii_punctuation())
        .collect::<String>();
    let mut hasher = Sha256::new();
    for value in [
        account_id,
        &group_id.to_string(),
        &normalized,
        knowledge_revision,
        provider_revision,
        PERSONA,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn prediction_narration_cache_key(
    account_id: &str,
    app_version: &str,
    normalized_data: &Value,
    provider_revision: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        account_id,
        app_version,
        &normalized_data.to_string(),
        provider_revision,
        PERSONA,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn is_cacheable_faq(question: &str) -> bool {
    let question = question.trim();
    if question.is_empty() || question.chars().count() > 240 {
        return false;
    }
    let context_dependent = [
        "刚才", "上面", "前面", "今天", "昨天", "这条", "这张", "总结", "摘要", "任务", "提醒",
        "预测", "处罚", "禁言", "撤回", "移出",
    ];
    !context_dependent.iter().any(|word| question.contains(word))
}

#[async_trait]
impl AiProvider for ConfiguredProvider {
    async fn decide(&self, request: &AiRequest) -> AppResult<AiDecision> {
        validate_request(request)?;
        if self.config.webhook_url.trim().is_empty() {
            match self.config.api_backend.as_str() {
                "responses" => {
                    // Reserve the first Responses probe. Concurrent AI assistant
                    // and semantic-classifier requests must not both spend their
                    // budget probing the same slow route.
                    if self
                        .responses_route_unavailable
                        .swap(true, Ordering::AcqRel)
                    {
                        return self.decide_openai(request).await;
                    }
                    let response = match tokio::time::timeout(
                        RESPONSES_ROUTE_PROBE_TIMEOUT,
                        self.decide_responses(request),
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(_) => Err(AppError::new(
                            "ai_responses_route",
                            "AI Responses 路由探测超时，已切换兼容调用",
                        )
                        .retryable()),
                    };
                    if response
                        .as_ref()
                        .err()
                        .is_some_and(responses_route_fallback_error)
                    {
                        // Some OpenAI-compatible gateways advertise Responses but
                        // route the configured model only through Chat Completions.
                        // Keep Responses as the configured contract and fall back
                        // only for route-level 404/502/503 errors.
                        self.decide_openai(request).await
                    } else {
                        if response.is_ok() {
                            self.responses_route_unavailable
                                .store(false, Ordering::Release);
                        }
                        response
                    }
                }
                _ => self.decide_openai(request).await,
            }
        } else {
            self.decide_webhook(request).await
        }
    }
}

fn responses_route_fallback_error(error: &AppError) -> bool {
    error.code == "ai_responses_route"
        || (error.code == "ai_http"
            && ["HTTP 404", "HTTP 502", "HTTP 503"]
                .iter()
                .any(|status| error.message.contains(status)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDecision {
    #[serde(default)]
    reply: String,
    #[serde(default)]
    actions: Vec<WireAction>,
    #[serde(default)]
    tasks: Vec<WireTask>,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAction {
    #[serde(rename = "type", alias = "kind")]
    kind: String,
    #[serde(default, rename = "durationSeconds", alias = "duration_seconds")]
    duration_seconds: i64,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTask {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "dueAt")]
    due_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<WireDecision> for AiDecision {
    fn from(value: WireDecision) -> Self {
        Self {
            reply: value.reply,
            actions: value
                .actions
                .into_iter()
                .map(|action| RuleAction {
                    kind: action.kind,
                    duration_seconds: action.duration_seconds,
                    message: action.message,
                })
                .collect(),
            tasks: value
                .tasks
                .into_iter()
                .map(|task| AiTask {
                    title: task.title,
                    description: task.description,
                    due_at: task.due_at,
                })
                .collect(),
            confidence: value.confidence,
            reason: value.reason,
        }
    }
}

pub fn decode_decision(content: &str) -> AppResult<AiDecision> {
    let wire: WireDecision = serde_json::from_str(content.trim())
        .map_err(|error| AppError::new("ai_response", format!("解析 AI 返回内容失败：{error}")))?;
    validate_decision(wire.into())
}

pub fn validate_request(request: &AiRequest) -> AppResult<()> {
    if request.version != "1" {
        return Err(AppError::new("ai_request", "AI 请求版本不受支持"));
    }
    if request.event_id.trim().is_empty() || request.message_id.trim().is_empty() {
        return Err(AppError::new("ai_request", "AI 请求缺少事件或消息标识"));
    }
    if request.message.trim().is_empty() {
        return Err(AppError::new("ai_request", "AI 测试内容为空"));
    }
    if request.message.chars().count() > 16_000 {
        return Err(AppError::new("ai_request", "AI 请求内容过长"));
    }
    if request.recent_context.len() > 200 || request.knowledge.len() > 32 {
        return Err(AppError::new("ai_request", "AI 上下文超出数量限制"));
    }
    if request
        .recent_context
        .iter()
        .any(|message| message.text.chars().count() > 16_000)
        || request
            .knowledge
            .iter()
            .any(|chunk| chunk.text.chars().count() > 8_000)
    {
        return Err(AppError::new("ai_request", "AI 上下文单项内容过长"));
    }
    Ok(())
}

pub fn validate_decision(decision: AiDecision) -> AppResult<AiDecision> {
    if !decision.confidence.is_finite() || !(0.0..=1.0).contains(&decision.confidence) {
        return Err(AppError::new("ai_response", "AI 置信度必须在 0 到 1 之间"));
    }
    if decision.reply.chars().count() > 4_000
        || decision.actions.len() > 32
        || decision.tasks.len() > 32
    {
        return Err(AppError::new(
            "ai_response",
            "AI 返回内容超出数量或长度限制",
        ));
    }
    let allowed = [
        "recall",
        "mute",
        "unmute",
        "remove",
        "rename",
        "blacklist",
        "ignore",
        "reply",
    ];
    for action in &decision.actions {
        if !allowed.contains(&action.kind.as_str()) {
            return Err(AppError::new(
                "ai_response",
                format!("AI 动作类型 {} 不受支持", action.kind),
            ));
        }
        if action.kind == "mute" && !(60..=2_592_000).contains(&action.duration_seconds) {
            return Err(AppError::new(
                "ai_response",
                "AI 禁言时长必须在 1 分钟到 30 天之间",
            ));
        }
        if action.message.chars().count() > 4_000 {
            return Err(AppError::new("ai_response", "AI 动作消息过长"));
        }
    }
    if decision
        .tasks
        .iter()
        .any(|task| task.title.trim().is_empty())
    {
        return Err(AppError::new("ai_response", "AI 任务缺少标题"));
    }
    Ok(decision)
}

pub fn is_mentioned(text: &str) -> bool {
    mention_metadata(text).is_some()
}

fn completion_url(raw: &str) -> AppResult<String> {
    endpoint_url(raw, "/chat/completions")
}

fn responses_url(raw: &str) -> AppResult<String> {
    endpoint_url(raw, "/responses")
}

fn endpoint_url(raw: &str, endpoint: &str) -> AppResult<String> {
    validate_remote_url(raw)?;
    let mut value = raw.trim().trim_end_matches('/').to_string();
    if value.ends_with(endpoint) {
        return Ok(value);
    }
    if !value.ends_with("/v1") {
        value.push_str("/v1");
    }
    value.push_str(endpoint);
    Ok(value)
}

pub fn normalize_api_backend(value: &str) -> AppResult<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "chat" | "chat_completions" | "openai" | "openai_compatible" => {
            Ok("chat_completions".into())
        }
        "responses" => Ok("responses".into()),
        _ => Err(AppError::new(
            "ai_backend",
            "AI 接口方式仅支持 Chat Completions 或 Responses",
        )),
    }
}

pub fn normalize_reasoning_effort(value: &str) -> AppResult<String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "low" => Ok("low".into()),
        "medium" | "high" | "xhigh" => Ok(normalized),
        "xhight" => Ok("xhigh".into()),
        _ => Err(AppError::new(
            "ai_reasoning_effort",
            "思考深度仅支持 low、medium、high 或 xhigh",
        )),
    }
}

fn response_instruction(persona: &str) -> String {
    format!(
        "{persona}\n\n只返回一个 JSON 对象，字段只能是 reply、actions、tasks、confidence、reason。群聊回复不得泄露接口地址、认证信息、上游字段和原始响应结构。"
    )
}

fn response_output_text(envelope: &Value) -> Option<&str> {
    envelope
        .get("output_text")
        .and_then(Value::as_str)
        .or_else(|| {
            envelope
                .get("output")
                .and_then(Value::as_array)
                .and_then(|output| {
                    output.iter().find_map(|item| {
                        item.get("content")
                            .and_then(Value::as_array)
                            .and_then(|content| {
                                content.iter().find_map(|part| {
                                    part.get("text").and_then(Value::as_str).or_else(|| {
                                        part.pointer("/text/value").and_then(Value::as_str)
                                    })
                                })
                            })
                    })
                })
        })
        .or_else(|| {
            envelope
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
        })
}

fn validate_remote_url(raw: &str) -> AppResult<()> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    let parsed = url::Url::parse(raw.trim())
        .map_err(|error| AppError::new("url_policy", format!("服务地址无效：{error}")))?;
    let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if parsed.scheme() == "https" || parsed.scheme() == "http" && loopback {
        Ok(())
    } else {
        Err(AppError::new(
            "url_policy",
            "远程服务必须使用 HTTPS，本机 HTTP 只允许回环地址",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn mock_openai(status: u16, reply: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = reply.to_string();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 16 * 1024];
            let _ = stream.read(&mut request);
            let body = if status == 200 {
                serde_json::json!({
                    "choices": [{
                        "message": {
                            "content": serde_json::json!({
                                "reply": reply,
                                "actions": [],
                                "tasks": [],
                                "confidence": 0.8,
                                "reason": "mock"
                            }).to_string()
                        }
                    }]
                })
                .to_string()
            } else {
                "{}".into()
            };
            let reason = if status == 200 {
                "OK"
            } else {
                "Service Unavailable"
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/v1"), worker)
    }

    fn mock_responses(reply: &str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = reply.to_string();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 16 * 1024];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("POST /v1/responses "));
            assert!(request.contains("\"max_output_tokens\":1024"));
            assert!(request.contains("\"reasoning\":{\"effort\":\"xhigh\"}"));
            let body = serde_json::json!({
                "output": [{
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": serde_json::json!({
                            "reply": reply,
                            "actions": [],
                            "tasks": [],
                            "confidence": 0.8,
                            "reason": "mock"
                        }).to_string()
                    }]
                }]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/v1"), worker)
    }
    #[test]
    fn mention_requires_at_sign_and_boundary() {
        assert!(is_mentioned("@DH 帮我看看"));
        assert!(is_mentioned("@ DH 预测"));
        assert!(!is_mentioned("DH 帮我"));
        assert!(!is_mentioned("@DHbot"));
    }
    #[test]
    fn base_url_is_normalized_once() {
        assert_eq!(
            completion_url("https://example.com").unwrap(),
            "https://example.com/v1/chat/completions"
        );
        assert_eq!(
            completion_url("https://example.com/v1").unwrap(),
            "https://example.com/v1/chat/completions"
        );
        assert_eq!(
            responses_url("https://example.com/v1").unwrap(),
            "https://example.com/v1/responses"
        );
        assert_eq!(normalize_api_backend("responses").unwrap(), "responses");
        assert_eq!(normalize_api_backend("openai").unwrap(), "chat_completions");
        assert_eq!(normalize_reasoning_effort("xhight").unwrap(), "xhigh");
    }
    #[test]
    fn remote_http_is_rejected() {
        assert!(completion_url("http://example.com/v1").is_err());
        assert!(completion_url("http://127.0.0.1:8000/v1").is_ok());
    }

    #[test]
    fn testing_request_only_includes_builtin_knowledge_when_selected() {
        let empty = AiRequest::testing("@DH 群规是什么", Vec::new());
        assert!(empty.knowledge.is_empty());
        let with_defaults = AiRequest::testing_with_knowledge("@DH 群规是什么", Vec::new(), true);
        assert_eq!(
            with_defaults.knowledge.len(),
            defaults::DEFAULT_KNOWLEDGE_DOCUMENTS.len()
        );
        assert!(with_defaults
            .knowledge
            .iter()
            .all(|chunk| chunk.base == defaults::DEFAULT_KNOWLEDGE_BASE_NAME));
    }
    #[test]
    fn unknown_response_fields_are_rejected() {
        assert!(decode_decision(
            r#"{"reply":"ok","actions":[],"tasks":[],"confidence":0.5,"reason":"ok","extra":1}"#
        )
        .is_err());
    }

    #[test]
    fn mention_metadata_preserves_exact_alias_and_byte_offsets() {
        let text = "前缀 @ DH 预测";
        let metadata = mention_metadata(text).unwrap();
        assert_eq!(metadata.alias, "@ DH");
        assert_eq!(&text[metadata.start..metadata.end], "@ DH");
        assert_eq!(message_without_mention(text), "前缀  预测");
    }

    #[test]
    fn mention_detection_is_case_insensitive_but_rejects_identifier_suffixes() {
        assert!(is_mentioned("@dh 你好"));
        assert!(is_mentioned("请 @Dh 帮忙"));
        for text in ["DH", "@DH_bot", "@DH2", "预测"] {
            assert!(!is_mentioned(text), "unexpected mention: {text}");
        }
    }

    fn context_message(id: i64, sequence: i64, seconds: i64, text: &str) -> Message {
        let sent_at = chrono::DateTime::from_timestamp(seconds, 0).unwrap();
        Message {
            id,
            account_id: "a".into(),
            group_id: 1,
            server_message_id: format!("m{id}"),
            sequence,
            user_id: id,
            sender_name: format!("成员{id}"),
            kind: "text".into(),
            text: text.into(),
            sent_at,
            received_at: sent_at,
            processed_at: None,
            acknowledged_at: None,
            processing_state: "done".into(),
            attempts: 0,
            next_attempt_at: None,
            last_error: String::new(),
            mentions_json: "[]".into(),
            source_kind: Some("text".into()),
            flow: Some("in".into()),
        }
    }

    #[test]
    fn recent_context_is_chronological_and_bounded() {
        let messages = vec![
            context_message(3, 3, 30, "third"),
            context_message(1, 1, 10, "first"),
            context_message(2, 2, 20, "second"),
        ];
        assert_eq!(
            build_recent_context(&messages, 2)
                .into_iter()
                .map(|item| item.text)
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
        assert!(build_recent_context(&messages, 0).is_empty());
    }

    fn text_decision(reply: &str) -> AiDecision {
        AiDecision {
            reply: reply.into(),
            actions: Vec::new(),
            tasks: Vec::new(),
            confidence: 0.8,
            reason: "test".into(),
        }
    }

    #[test]
    fn provider_pool_reuses_client_for_the_same_configuration() {
        let pool = AiProviderPool::default();
        let config = AiConfig {
            base_url: "http://127.0.0.1:18080/v1".into(),
            webhook_url: String::new(),
            api_backend: "chat_completions".into(),
            model: "deepseek-v4-pro".into(),
            reasoning_effort: "low".into(),
            api_key: "TOKEN".into(),
            timeout: Duration::from_secs(8),
        };
        let first = pool.provider(config.clone()).unwrap();
        let second = pool.provider(config).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn responses_backend_accepts_the_dh_decision_schema() {
        let (base_url, worker) = mock_responses("responses reply");
        let provider = ConfiguredProvider::new(AiConfig {
            base_url,
            webhook_url: String::new(),
            api_backend: "responses".into(),
            model: "gpt-5.6-luna".into(),
            reasoning_effort: "xhight".into(),
            api_key: "TOKEN".into(),
            timeout: Duration::from_secs(2),
        })
        .unwrap();
        let decision = provider
            .decide(&AiRequest::testing("@DH 你好", Vec::new()))
            .await
            .unwrap();
        assert_eq!(decision.reply, "responses reply");
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn provider_chain_cools_failed_primary_and_uses_one_backup() {
        let (primary_url, primary_worker) = mock_openai(503, "");
        let (backup_url, backup_worker) = mock_openai(200, "backup reply");
        let primary = AiConfig {
            base_url: primary_url,
            webhook_url: String::new(),
            api_backend: "chat_completions".into(),
            model: "deepseek-v4-pro".into(),
            reasoning_effort: "low".into(),
            api_key: "PRIMARY".into(),
            timeout: Duration::from_secs(2),
        };
        let backup = AiConfig {
            base_url: backup_url,
            webhook_url: String::new(),
            api_backend: "chat_completions".into(),
            model: "deepseek-v4-pro".into(),
            reasoning_effort: "low".into(),
            api_key: "BACKUP".into(),
            timeout: Duration::from_secs(2),
        };
        let primary_fingerprint = config_fingerprint(&primary);
        let pool = AiProviderPool::default();
        let chain = pool.chain(vec![primary, backup]).unwrap();
        let decision = chain
            .decide(&AiRequest::testing("@DH 群规是什么", Vec::new()))
            .await
            .unwrap();
        assert_eq!(decision.reply, "backup reply");
        assert_eq!(chain.last_attempt_count(), 2);
        assert!(!pool.available(&primary_fingerprint));
        primary_worker.join().unwrap();
        backup_worker.join().unwrap();
    }

    #[test]
    fn reply_cache_only_accepts_plain_text_decisions() {
        let cache = AiReplyCache::new(2, Duration::from_secs(60));
        cache.insert_scoped("account", "one".into(), text_decision("cached"));
        assert_eq!(cache.get_scoped("account", "one").unwrap().reply, "cached");

        let mut action = text_decision("must-not-cache");
        action.actions.push(RuleAction {
            kind: "recall".into(),
            duration_seconds: 0,
            message: String::new(),
        });
        cache.insert_scoped("account", "action".into(), action);
        assert!(cache.get_scoped("account", "action").is_none());

        cache.insert_scoped("account", "two".into(), text_decision("two"));
        cache.insert_scoped("account", "three".into(), text_decision("three"));
        assert!(cache.get_scoped("account", "one").is_none());
    }

    #[test]
    fn reply_cache_capacity_is_isolated_per_account() {
        let cache = AiReplyCache::new(1, Duration::from_secs(60));
        cache.insert_scoped("account-a", "one".into(), text_decision("a"));
        cache.insert_scoped("account-b", "one".into(), text_decision("b"));
        assert_eq!(cache.get_scoped("account-a", "one").unwrap().reply, "a");
        assert_eq!(cache.get_scoped("account-b", "one").unwrap().reply, "b");
    }

    #[test]
    fn request_budget_limits_context_and_knowledge() {
        let mut request = AiRequest::testing("@DH test", Vec::new());
        request.recent_context = (0..20)
            .map(|index| AiContextMessage {
                user_id: index,
                name: format!("member-{index}"),
                text: "x".repeat(600),
                time: index,
            })
            .collect();
        request.knowledge = (0..8)
            .map(|index| AiKnowledgeChunk {
                base: "base".into(),
                title: format!("doc-{index}"),
                source: "test".into(),
                text: "知".repeat(1_500),
            })
            .collect();
        let bounded = bounded_request(&request);
        assert!(bounded.recent_context.len() <= 8);
        assert!(bounded.knowledge.len() <= 3);
        assert!(
            bounded
                .recent_context
                .iter()
                .map(|item| item.text.chars().count())
                .sum::<usize>()
                <= MAX_RECENT_CHARS
        );
        assert!(
            bounded
                .knowledge
                .iter()
                .map(|item| item.text.chars().count())
                .sum::<usize>()
                <= MAX_KNOWLEDGE_CHARS
        );
    }

    #[test]
    fn prediction_narration_cache_key_tracks_period_algorithm_and_provider() {
        let first = prediction_narration_cache_key(
            "account",
            "1.0.0",
            &serde_json::json!({"game":"PC28","period":"100"}),
            "provider-a",
        );
        let same = prediction_narration_cache_key(
            "account",
            "1.0.0",
            &serde_json::json!({"game":"PC28","period":"100"}),
            "provider-a",
        );
        let next_period = prediction_narration_cache_key(
            "account",
            "1.0.0",
            &serde_json::json!({"game":"PC28","period":"101"}),
            "provider-a",
        );
        assert_eq!(first, same);
        assert_ne!(first, next_period);
    }

    #[tokio::test]
    #[ignore = "requires DH_AI_LIVE_URL and DH_AI_LIVE_KEY"]
    async fn live_openai_compatible_provider_accepts_dh_schema() {
        let base_url = std::env::var("DH_AI_LIVE_URL").expect("DH_AI_LIVE_URL is required");
        let api_key = std::env::var("DH_AI_LIVE_KEY").expect("DH_AI_LIVE_KEY is required");
        let message = std::env::var("DH_AI_LIVE_MESSAGE")
            .unwrap_or_else(|_| "@DH 请用一句自然中文介绍你能做什么。".into());
        let provider = ConfiguredProvider::new(AiConfig {
            base_url,
            webhook_url: String::new(),
            api_backend: std::env::var("DH_AI_LIVE_BACKEND")
                .unwrap_or_else(|_| "chat_completions".into()),
            model: std::env::var("DH_AI_LIVE_MODEL").unwrap_or_else(|_| "deepseek-v4-pro".into()),
            reasoning_effort: std::env::var("DH_AI_LIVE_REASONING")
                .unwrap_or_else(|_| "low".into()),
            api_key,
            timeout: Duration::from_secs(30),
        })
        .expect("live provider configuration should be valid");
        let result = provider
            .test(&AiRequest::testing_with_knowledge(
                message,
                Vec::new(),
                true,
            ))
            .await
            .expect("live provider should return a valid DH decision");

        assert!(!result.decision.reply.trim().is_empty());
        assert!((0.0..=1.0).contains(&result.decision.confidence));
        eprintln!(
            "live AI passed: model={}, elapsed_ms={}, reply_preview={:?}, action_kinds={:?}, tasks={}",
            result.model,
            result.elapsed_ms,
            result.decision.reply.chars().take(240).collect::<String>(),
            result
                .decision
                .actions
                .iter()
                .map(|action| action.kind.as_str())
                .collect::<Vec<_>>(),
            result.decision.tasks.len()
        );
    }
}
