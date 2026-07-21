use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{AiDecision, AiTask, RuleAction};

pub const PERSONA: &str = include_str!("../../../AGENTS.md");

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
            knowledge: Vec::new(),
        }
    }
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
        if request.message.trim().is_empty() {
            return Err(AppError::new("ai_request", "AI 测试内容为空"));
        }
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

fn decode_decision(content: &str) -> AppResult<AiDecision> {
    let wire: WireDecision = serde_json::from_str(content.trim())
        .map_err(|error| AppError::new("ai_response", format!("解析 AI 返回内容失败：{error}")))?;
    validate_decision(wire.into())
}

fn validate_decision(decision: AiDecision) -> AppResult<AiDecision> {
    if !(0.0..=1.0).contains(&decision.confidence) {
        return Err(AppError::new("ai_response", "AI 置信度必须在 0 到 1 之间"));
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
    let normalized = text.to_lowercase().replace("@ dh", "@dh");
    let bytes = normalized.as_bytes();
    let mut offset = 0;
    while let Some(index) = normalized[offset..].find("@dh") {
        let end = offset + index + 3;
        if end == bytes.len() || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_' {
            return true;
        }
        offset = end;
    }
    false
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
    fn unknown_response_fields_are_rejected() {
        assert!(decode_decision(
            r#"{"reply":"ok","actions":[],"tasks":[],"confidence":0.5,"reason":"ok","extra":1}"#
        )
        .is_err());
    }
}
