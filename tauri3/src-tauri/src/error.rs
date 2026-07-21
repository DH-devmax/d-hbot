use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GatewayErrorLayer {
    Transport,
    Business,
    Response,
    Delivery,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GatewayErrorKind {
    Transport,
    NimNotReady,
    Permission,
    NotFound,
    Business,
    Unsupported,
    Decode,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayErrorMetadata {
    pub route: String,
    pub layer: GatewayErrorLayer,
    pub kind: GatewayErrorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_errno: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_errno: Option<i64>,
}

impl GatewayErrorMetadata {
    pub fn new(route: impl Into<String>, layer: GatewayErrorLayer) -> Self {
        Self {
            route: route.into(),
            layer,
            kind: GatewayErrorKind::Unknown,
            transport_code: None,
            transport_errno: None,
            business_code: None,
            business_errno: None,
        }
    }

    pub fn with_kind(mut self, kind: GatewayErrorKind) -> Self {
        self.kind = kind;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<Box<GatewayErrorMetadata>>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            detail_ref: None,
            gateway: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn with_detail(mut self, detail_ref: impl Into<String>) -> Self {
        self.detail_ref = Some(detail_ref.into());
        self
    }

    pub fn with_gateway(mut self, metadata: GatewayErrorMetadata) -> Self {
        self.gateway = Some(Box::new(metadata));
        self
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

#[derive(Debug, Error)]
pub enum InternalError {
    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("文件错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("网络错误：{0}")]
    Network(#[from] reqwest::Error),
    #[error("数据格式错误：{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

impl From<InternalError> for AppError {
    fn from(value: InternalError) -> Self {
        match value {
            InternalError::Database(_) => AppError::new("database_error", value.to_string()),
            InternalError::Io(_) => AppError::new("io_error", value.to_string()),
            InternalError::Network(_) => {
                AppError::new("network_error", value.to_string()).retryable()
            }
            InternalError::Json(_) => AppError::new("invalid_data", value.to_string()),
            InternalError::Message(message) => AppError::new("operation_failed", message),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
