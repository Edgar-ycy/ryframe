use std::fmt;

use serde::{Deserialize, Serialize};
use validator::ValidationErrors;

/// 可长期稳定使用的领域错误码。
///
/// 序列化键是当前唯一的跨进程契约；已有键不得改名或复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Validation,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    PayloadTooLarge,
    RateLimited,
    Database,
    Config,
    Internal,
    ServiceUnavailable,
}

impl ErrorCode {
    /// 返回稳定的机器可读错误键。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::RateLimited => "rate_limited",
            Self::Database => "database",
            Self::Config => "config",
            Self::Internal => "internal",
            Self::ServiceUnavailable => "service_unavailable",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 与传输协议无关的应用错误。
///
/// HTTP 状态码、响应信封与日志脱敏应由边缘适配层处理，领域层仅表达失败语义及细节。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("参数校验失败: {0}")]
    Validation(String),
    #[error("认证失败: {0}")]
    Authentication(String),
    #[error("权限不足: {0}")]
    Authorization(String),
    #[error("资源不存在: {0}")]
    NotFound(String),
    #[error("数据冲突: {0}")]
    Conflict(String),
    #[error("请求体过大: {0}")]
    PayloadTooLarge(String),
    #[error("请求过于频繁: {0}")]
    RateLimited(String, u64),
    #[error("数据库错误: {0}")]
    Database(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("内部错误: {0}")]
    Internal(String),
    #[error("服务暂不可用: {0}")]
    ServiceUnavailable(String),
}

impl AppError {
    /// 返回错误对应的稳定领域错误码。
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::Authentication(_) => ErrorCode::Authentication,
            Self::Authorization(_) => ErrorCode::Authorization,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::PayloadTooLarge(_) => ErrorCode::PayloadTooLarge,
            Self::RateLimited(_, _) => ErrorCode::RateLimited,
            Self::Database(_) => ErrorCode::Database,
            Self::Config(_) => ErrorCode::Config,
            Self::Internal(_) => ErrorCode::Internal,
            Self::ServiceUnavailable(_) => ErrorCode::ServiceUnavailable,
        }
    }

    /// 返回适合领域层记录或进一步适配的原始错误消息。
    pub fn message(&self) -> &str {
        match self {
            Self::Validation(message)
            | Self::Authentication(message)
            | Self::Authorization(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::PayloadTooLarge(message)
            | Self::Database(message)
            | Self::Config(message)
            | Self::Internal(message)
            | Self::ServiceUnavailable(message) => message,
            Self::RateLimited(message, _) => message,
        }
    }

    /// 返回限流错误的重试等待秒数，其他错误返回 `None`。
    pub const fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited(_, retry_after_secs) => Some(*retry_after_secs),
            _ => None,
        }
    }
}

impl From<ValidationErrors> for AppError {
    fn from(error: ValidationErrors) -> Self {
        Self::Validation(error.to_string())
    }
}
