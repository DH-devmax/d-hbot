use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
    pub model: String,
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
}

pub struct ConfiguredProvider {
    config: AiConfig,
    client: reqwest::Client,
}

impl ConfiguredProvider {
    pub fn new(mut config: AiConfig) -> AppResult<Self> {
        if config.model.trim().is_empty() {
            config.model = "deepseek-v4-pro".into();
        }
        if config.timeout.is_zero() {
            config.timeout = Duration::from_secs(30);
        }
        validate_remote_url(if config.webhook_url.trim().is_empty() {
            &config.base_url
        } else {
            &config.webhook_url
        })?;
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| AppError::new("ai_client", error.to_string()))?;
        Ok(Self { config, client })
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
        let request_json = serde_json::to_string(request)
            .map_err(|error| AppError::new("ai_request", error.to_string()))?;
        let body = json!({
            "model": self.config.model,
            "temperature": 0.3,
            "response_format": {"type":"json_object"},
            "messages": [
                {"role":"system","content":format!("{}\n\n只返回一个 JSON 对象，字段只能是 reply、actions、tasks、confidence、reason。群聊回复不得泄露接口地址、认证信息、上游字段和原始响应结构。", PERSONA)},
                {"role":"user","content":request_json}
            ]
        });
        let mut outbound = self.client.post(url).json(&body);
        if !self.config.api_key.trim().is_empty() {
            outbound = outbound.bearer_auth(self.config.api_key.trim());
        }
        let response = outbound.send().await.map_err(|error| {
            AppError::new("ai_request", format!("AI 请求失败：{error}")).retryable()
        })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AppError::new("ai_response", error.to_string()))?;
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

    async fn decide_webhook(&self, request: &AiRequest) -> AppResult<AiDecision> {
        let response = self
            .client
            .post(self.config.webhook_url.trim())
            .json(request)
            .send()
            .await
            .map_err(|error| {
                AppError::new("webhook_request", format!("Webhook 请求失败：{error}")).retryable()
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                AppError::new("webhook_http", format!("Webhook 返回 HTTP {status}")).retryable(),
            );
        }
        let wire: WireDecision = response.json().await.map_err(|error| {
            AppError::new("webhook_response", format!("Webhook 响应格式错误：{error}"))
        })?;
        validate_decision(wire.into())
    }
}

#[async_trait]
impl AiProvider for ConfiguredProvider {
    async fn decide(&self, request: &AiRequest) -> AppResult<AiDecision> {
        validate_request(request)?;
        if self.config.webhook_url.trim().is_empty() {
            self.decide_openai(request).await
        } else {
            self.decide_webhook(request).await
        }
    }
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
    validate_remote_url(raw)?;
    let mut value = raw.trim().trim_end_matches('/').to_string();
    if value.ends_with("/chat/completions") {
        return Ok(value);
    }
    if !value.ends_with("/v1") {
        value.push_str("/v1");
    }
    value.push_str("/chat/completions");
    Ok(value)
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
}
