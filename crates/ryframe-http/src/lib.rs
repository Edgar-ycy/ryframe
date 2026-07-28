//! HTTP 边界适配层。
//!
//! 领域代码只依赖 `ryframe-kernel` 中的错误和上下文类型；本库负责把领域错误
//! 映射为 Axum 响应，并提供稳定的 REST 响应结构。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ryframe_kernel::{AppError as KernelAppError, ErrorCode};
use serde::Serialize;
use utoipa::ToSchema;
use validator::ValidationErrors;

/// HTTP 调用方使用的兼容错误类型。
///
/// 新的领域代码应直接使用 `ryframe_kernel::AppError`。该类型保留相同变体，
/// 以维持 Axum 处理器的 `IntoResponse` 契约，并可与内核错误双向转换。
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
    /// 返回与内核错误一致的稳定错误码。
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

    /// 将 HTTP 兼容错误转换为纯领域错误。
    pub fn into_kernel(self) -> KernelAppError {
        self.into()
    }
}

impl From<AppError> for KernelAppError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::Validation(message) => Self::Validation(message),
            AppError::Authentication(message) => Self::Authentication(message),
            AppError::Authorization(message) => Self::Authorization(message),
            AppError::NotFound(message) => Self::NotFound(message),
            AppError::Conflict(message) => Self::Conflict(message),
            AppError::PayloadTooLarge(message) => Self::PayloadTooLarge(message),
            AppError::RateLimited(message, retry_after) => Self::RateLimited(message, retry_after),
            AppError::Database(message) => Self::Database(message),
            AppError::Config(message) => Self::Config(message),
            AppError::Internal(message) => Self::Internal(message),
            AppError::ServiceUnavailable(message) => Self::ServiceUnavailable(message),
        }
    }
}

impl From<KernelAppError> for AppError {
    fn from(error: KernelAppError) -> Self {
        match error {
            KernelAppError::Validation(message) => Self::Validation(message),
            KernelAppError::Authentication(message) => Self::Authentication(message),
            KernelAppError::Authorization(message) => Self::Authorization(message),
            KernelAppError::NotFound(message) => Self::NotFound(message),
            KernelAppError::Conflict(message) => Self::Conflict(message),
            KernelAppError::PayloadTooLarge(message) => Self::PayloadTooLarge(message),
            KernelAppError::RateLimited(message, retry_after) => {
                Self::RateLimited(message, retry_after)
            }
            KernelAppError::Database(message) => Self::Database(message),
            KernelAppError::Config(message) => Self::Config(message),
            KernelAppError::Internal(message) => Self::Internal(message),
            KernelAppError::ServiceUnavailable(message) => Self::ServiceUnavailable(message),
        }
    }
}

impl From<ValidationErrors> for AppError {
    fn from(error: ValidationErrors) -> Self {
        KernelAppError::from(error).into()
    }
}

/// HTTP 处理器的统一结果类型。
pub type AppResult<T> = Result<T, AppError>;

/// 统一 API 响应结构。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponse<T: Serialize> {
    /// 与 HTTP 状态码一致的业务结果码。
    pub code: u16,
    /// 面向用户的可读消息。
    pub message: String,
    /// 业务响应数据；无数据时显式返回 `null`。
    pub data: Option<T>,
    /// 与 `X-Request-Id` 响应头一致的 UUID v7。
    pub request_id: String,
    /// 面向程序处理的稳定错误键；成功时为 `null`。
    pub error_key: Option<String>,
    /// 可安全公开的结构化错误参数；无参数时为 `null`。
    pub details: Option<serde_json::Value>,
}

/// 不携带业务数据的统一响应。
///
/// 保持独立类型可让 OpenAPI 正确生成空数据响应的 Schema，避免把 Rust 的
/// 单元类型错误地暴露成不存在的组件引用。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiEmptyResponse {
    pub code: u16,
    pub message: String,
    pub data: Option<()>,
    pub request_id: String,
    pub error_key: Option<String>,
    pub details: Option<serde_json::Value>,
}

impl ApiEmptyResponse {
    /// 构造默认消息为“操作成功”的空数据响应。
    pub fn success_no_data() -> Self {
        Self {
            code: 200,
            message: "操作成功".into(),
            data: None,
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }

    /// 构造带自定义消息的空数据响应。
    pub fn success_no_data_with_msg(message: impl Into<String>) -> Self {
        Self {
            code: 200,
            message: message.into(),
            data: None,
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }
}

impl<T: Serialize> ApiResponse<T> {
    /// 构造默认消息为“操作成功”的成功响应。
    pub fn success(data: T) -> Self {
        Self {
            code: 200,
            message: "操作成功".into(),
            data: Some(data),
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }

    /// 构造带自定义消息的成功响应。
    pub fn success_msg(message: impl Into<String>, data: T) -> Self {
        Self {
            code: 200,
            message: message.into(),
            data: Some(data),
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }
}

impl ApiResponse<()> {
    /// 构造默认消息且不带数据的成功响应。
    pub fn success_no_data() -> Self {
        Self {
            code: 200,
            message: "操作成功".into(),
            data: None,
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }

    /// 构造带自定义消息且不带数据的成功响应。
    pub fn success_no_data_with_msg(message: impl Into<String>) -> Self {
        Self {
            code: 200,
            message: message.into(),
            data: None,
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }

    /// 构造失败响应。
    pub fn fail(code: u16, message: impl Into<String>, error_key: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
            request_id: String::new(),
            error_key: Some(error_key.into()),
            details: None,
        }
    }
}

/// 分页接口的业务数据。
#[derive(Debug, Serialize, ToSchema)]
pub struct PageData<T: Serialize> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub max_page_size: u64,
}

/// 统一分页 API 响应结构。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiPageResponse<T: Serialize> {
    pub code: u16,
    pub message: String,
    pub data: PageData<T>,
    pub request_id: String,
    pub error_key: Option<String>,
    pub details: Option<serde_json::Value>,
}

impl<T: Serialize> ApiPageResponse<T> {
    /// 从分页数据构造带自定义消息的响应。
    pub fn new(
        items: Vec<T>,
        total: u64,
        page: u64,
        page_size: u64,
        max_page_size: u64,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: 200,
            message: message.into(),
            data: PageData {
                total_pages: total.div_ceil(page_size.max(1)),
                items,
                page,
                page_size,
                total,
                max_page_size,
            },
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }

    /// 从分页数据构造默认消息为“查询成功”的响应。
    pub fn page(items: Vec<T>, total: u64, page: u64, page_size: u64, max_page_size: u64) -> Self {
        Self::new(items, total, page, page_size, max_page_size, "查询成功")
    }
}

/// 将领域错误适配为 HTTP 响应的本地包装器。
///
/// `AppError` 位于 `ryframe-kernel`，因此领域库不实现 Axum 的 `IntoResponse`；
/// HTTP 边缘代码需要直接返回响应时，应使用本包装器。
#[derive(Debug)]
pub struct HttpAppError(pub KernelAppError);

impl From<KernelAppError> for HttpAppError {
    fn from(error: KernelAppError) -> Self {
        Self(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        HttpAppError::from(self.into_kernel()).into_response()
    }
}

impl IntoResponse for HttpAppError {
    fn into_response(self) -> Response {
        let is_prod = std::env::var("APP_ENV").is_ok_and(|env| {
            matches!(
                env.trim().to_ascii_lowercase().as_str(),
                "prod" | "production"
            )
        });
        let error = self.0;
        let (status, message, retry_after) = match &error {
            KernelAppError::Validation(message) => (StatusCode::BAD_REQUEST, message.clone(), None),
            KernelAppError::Authentication(message) => {
                (StatusCode::UNAUTHORIZED, message.clone(), None)
            }
            KernelAppError::Authorization(message) => {
                (StatusCode::FORBIDDEN, message.clone(), None)
            }
            KernelAppError::NotFound(message) => (StatusCode::NOT_FOUND, message.clone(), None),
            KernelAppError::Conflict(message) => (StatusCode::CONFLICT, message.clone(), None),
            KernelAppError::PayloadTooLarge(message) => {
                (StatusCode::PAYLOAD_TOO_LARGE, message.clone(), None)
            }
            KernelAppError::RateLimited(message, retry_after) => (
                StatusCode::TOO_MANY_REQUESTS,
                message.clone(),
                Some(*retry_after),
            ),
            KernelAppError::Database(message) => {
                tracing::error!(error = %message, "数据库错误");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "数据库服务暂不可用".to_string(),
                    None,
                )
            }
            KernelAppError::Config(message) => {
                tracing::error!(error = %message, "配置错误");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    internal_error_message(is_prod, message),
                    None,
                )
            }
            KernelAppError::Internal(message) => {
                tracing::error!(error = %message, "内部错误");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    internal_error_message(is_prod, message),
                    None,
                )
            }
            KernelAppError::ServiceUnavailable(message) => {
                tracing::error!(error = %message, "服务暂不可用");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    service_unavailable_error_message(is_prod, message),
                    None,
                )
            }
        };

        let body = ApiResponse::<()>::fail(status.as_u16(), message, error.error_code().as_str());
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| {
                r#"{"code":500,"message":"序列化错误响应失败","data":null,"request_id":"","error_key":"internal","details":null}"#.into()
            });
        let mut response = Response::new(axum::body::Body::from(json));
        *response.status_mut() = status;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        if let Some(retry_after) = retry_after
            && let Ok(value) = http::HeaderValue::from_str(&retry_after.max(1).to_string())
        {
            response
                .headers_mut()
                .insert(http::header::RETRY_AFTER, value);
        }
        response
    }
}

/// 将领域错误转换为当前 HTTP 兼容响应。
pub fn app_error_response(error: KernelAppError) -> Response {
    HttpAppError(error).into_response()
}

fn internal_error_message(is_prod: bool, detail: &str) -> String {
    if is_prod {
        "服务器内部错误".to_string()
    } else {
        detail.to_string()
    }
}

fn service_unavailable_error_message(is_prod: bool, detail: &str) -> String {
    if is_prod {
        "服务暂不可用".to_string()
    } else {
        detail.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::service_unavailable_error_message;

    #[test]
    fn production_service_unavailable_errors_do_not_expose_dependency_details() {
        let detail = "redis://user:secret@internal.example:6379 unavailable";

        assert_eq!(
            service_unavailable_error_message(true, detail),
            "服务暂不可用"
        );
        assert_eq!(service_unavailable_error_message(false, detail), detail);
    }
}
