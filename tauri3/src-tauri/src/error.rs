use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            detail_ref: None,
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
