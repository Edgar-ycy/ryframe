//! HTTP 边界适配层。
//!
//! 领域代码只依赖 `ryframe-kernel` 中的错误和上下文类型；本库负责把领域错误
//! 映射为 Axum 响应，并提供稳定的 REST 响应结构。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ryframe_kernel::{AppError, ErrorCode};
use serde::Serialize;
use std::fmt;
use validator::ValidationErrors;

/// 当前公开 API 的唯一 URL 前缀。
pub const API_PREFIX: &str = "/api/v1";

/// API 边缘渲染通用成功消息时使用的资源键。
pub const SUCCESS_MESSAGE_KEY: &str = "common.success";

/// API 边缘渲染分页查询成功消息时使用的资源键。
pub const QUERY_SUCCESS_MESSAGE_KEY: &str = "common.query";

/// 标记已由依赖状态边界确认的预期 503 响应。
///
/// 该标记只保存在响应扩展中，不会写入响应头或响应体。请求日志层据此保留
/// 状态码与指标，但不会把配置导致的预期不可用重复记录为请求失败。
#[derive(Clone, Copy, Debug)]
pub struct ExpectedServiceUnavailableResponse;

/// 为已确认的预期服务不可用响应添加内部日志标记。
///
/// 调用方必须已在依赖状态边界记录实际故障或禁用状态；HTTP 边界只负责把
/// 错误转换为稳定的 API 响应，不能在每个请求上重复输出 ERROR。
pub fn mark_expected_service_unavailable(response: &mut Response) {
    response
        .extensions_mut()
        .insert(ExpectedServiceUnavailableResponse);
}

/// 将稳定错误码映射为同名本地化资源键。
pub const fn error_message_key(error_code: ErrorCode) -> &'static str {
    match error_code {
        ErrorCode::Validation => "error.validation",
        ErrorCode::Authentication => "error.authentication",
        ErrorCode::Authorization => "error.authorization",
        ErrorCode::NotFound => "error.not_found",
        ErrorCode::Conflict => "error.conflict",
        ErrorCode::PayloadTooLarge => "error.payload_too_large",
        ErrorCode::RateLimited => "error.rate_limited",
        ErrorCode::Database => "error.database",
        ErrorCode::Config => "error.config",
        ErrorCode::Internal => "error.internal",
        ErrorCode::ServiceUnavailable => "error.service_unavailable",
        ErrorCode::CapabilityUnavailable => "error.capability_unavailable",
        ErrorCode::TenantCapabilityDenied => "error.tenant_capability_denied",
        ErrorCode::PermissionDenied => "error.permission_denied",
        ErrorCode::StaleRuntimeEpoch => "error.stale_runtime_epoch",
        ErrorCode::StalePlacementGeneration => "error.stale_placement_generation",
        ErrorCode::TenantOperationConflict => "error.tenant_operation_conflict",
        ErrorCode::TenantDataMaintenance => "error.tenant_data_maintenance",
        ErrorCode::TenantDataTargetUnavailable => "error.tenant_data_target_unavailable",
    }
}

/// 将不带版本的相对路径连接到唯一公开 API 前缀。
pub fn api_path(relative_path: &str) -> String {
    let relative_path = relative_path.trim_matches('/');
    if relative_path.is_empty() {
        API_PREFIX.to_owned()
    } else {
        format!("{API_PREFIX}/{relative_path}")
    }
}

/// 统一 API 响应结构。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiEmptyResponse {
    pub code: u16,
    pub message: String,
    pub data: Option<()>,
    pub request_id: String,
    pub error_key: Option<String>,
    pub details: Option<serde_json::Value>,
}

impl ApiEmptyResponse {
    /// 构造由 API 边缘渲染通用成功消息的空数据响应。
    pub fn success_no_data() -> Self {
        Self {
            code: 200,
            message: SUCCESS_MESSAGE_KEY.into(),
            data: None,
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }
}

impl<T: Serialize> ApiResponse<T> {
    /// 构造由 API 边缘渲染通用成功消息的成功响应。
    pub fn success(data: T) -> Self {
        Self {
            code: 200,
            message: SUCCESS_MESSAGE_KEY.into(),
            data: Some(data),
            request_id: String::new(),
            error_key: None,
            details: None,
        }
    }
}

impl ApiResponse<()> {
    /// 构造由 API 边缘渲染通用成功消息且不带数据的响应。
    pub fn success_no_data() -> Self {
        Self {
            code: 200,
            message: SUCCESS_MESSAGE_KEY.into(),
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
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageData<T: Serialize> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub max_page_size: u64,
}

/// 统一分页 API 响应结构。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiPageResponse<T: Serialize> {
    pub code: u16,
    pub message: String,
    pub data: PageData<T>,
    pub request_id: String,
    pub error_key: Option<String>,
    pub details: Option<serde_json::Value>,
}

impl<T: Serialize> ApiPageResponse<T> {
    /// 从分页数据构造由 API 边缘渲染查询成功消息的响应。
    pub fn page(items: Vec<T>, total: u64, page: u64, page_size: u64, max_page_size: u64) -> Self {
        Self {
            code: 200,
            message: QUERY_SUCCESS_MESSAGE_KEY.into(),
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
}

/// 将领域错误适配为 HTTP 响应的本地包装器。
///
/// `AppError` 位于 `ryframe-kernel`，因此领域库不实现 Axum 的 `IntoResponse`；
/// HTTP 边缘代码需要直接返回响应时，应使用本包装器。
#[derive(Debug)]
pub struct HttpAppError(pub AppError);

impl fmt::Display for HttpAppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for HttpAppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<AppError> for HttpAppError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

impl From<ValidationErrors> for HttpAppError {
    fn from(error: ValidationErrors) -> Self {
        Self(AppError::from(error))
    }
}

/// HTTP 处理器的统一结果类型。
pub type HttpResult<T> = Result<T, HttpAppError>;

impl IntoResponse for HttpAppError {
    fn into_response(self) -> Response {
        let error = self.0;
        let error_code = error.error_code();
        let (status, retry_after) = match &error {
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, None),
            AppError::Authentication(_) => (StatusCode::UNAUTHORIZED, None),
            AppError::Authorization(_) => (StatusCode::FORBIDDEN, None),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, None),
            AppError::Conflict(_) => (StatusCode::CONFLICT, None),
            AppError::RetryableConflict(_, retry_after) => {
                (StatusCode::CONFLICT, Some(*retry_after))
            }
            AppError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, None),
            AppError::RateLimited(_, retry_after) => {
                (StatusCode::TOO_MANY_REQUESTS, Some(*retry_after))
            }
            AppError::Database(message) => {
                tracing::error!(error = %message, "数据库错误");
                (StatusCode::SERVICE_UNAVAILABLE, None)
            }
            AppError::Config(message) => {
                tracing::error!(error = %message, "配置错误");
                (StatusCode::INTERNAL_SERVER_ERROR, None)
            }
            AppError::Internal(message) => {
                tracing::error!(error = %message, "内部错误");
                (StatusCode::INTERNAL_SERVER_ERROR, None)
            }
            // 依赖故障或显式禁用的日志应由拥有该依赖状态的边界负责；此处仅做
            // HTTP 映射，避免同一个 503 在每个请求上重复输出 ERROR。
            AppError::ServiceUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, None),
            AppError::CapabilityUnavailable(_) => (StatusCode::NOT_IMPLEMENTED, None),
            AppError::TenantCapabilityDenied(_) | AppError::PermissionDenied(_) => {
                (StatusCode::FORBIDDEN, None)
            }
            AppError::StaleRuntimeEpoch(_)
            | AppError::StalePlacementGeneration(_)
            | AppError::TenantOperationConflict(_) => (StatusCode::CONFLICT, None),
            AppError::TenantDataMaintenance(_, retry_after) => {
                (StatusCode::LOCKED, Some(*retry_after))
            }
            AppError::TenantDataTargetUnavailable(_, retry_after) => {
                (StatusCode::SERVICE_UNAVAILABLE, Some(*retry_after))
            }
        };

        let body = ApiResponse::<()>::fail(
            status.as_u16(),
            error_message_key(error_code),
            error_code.as_str(),
        );
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| {
                r#"{"code":500,"message":"error.internal","data":null,"request_id":"","error_key":"internal","details":null}"#.into()
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

/// 将领域错误转换为统一 HTTP 响应。
pub fn app_error_response(error: AppError) -> Response {
    HttpAppError(error).into_response()
}
