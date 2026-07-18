use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use validator::ValidationErrors;
// 统一错误类型 (AppError)
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
/// 统一 API 响应结构体
///
/// 成功响应（单对象/列表）：{"code": 200, "msg": "操作成功", "data": {...}}
/// 成功响应（无数据）：{"code": 200, "msg": "操作成功"}
/// 错误响应：{"code": 400, "msg": "参数校验失败: xxx"}
///
/// 分页查询请使用 [`ApiPageResponse`]。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

/// 成功且不携带 `data` 字段的响应契约。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiEmptyResponse {
    pub code: i32,
    pub msg: String,
}

impl<T: Serialize> ApiResponse<T> {
    /// 成功响应（默认消息"操作成功"）
    pub fn success(data: T) -> Self {
        Self {
            code: 200,
            msg: "操作成功".into(),
            data: Some(data),
        }
    }

    /// 成功响应（自定义消息）
    pub fn success_msg(msg: impl Into<String>, data: T) -> Self {
        Self {
            code: 200,
            msg: msg.into(),
            data: Some(data),
        }
    }
}

impl ApiResponse<()> {
    /// 成功响应（无数据，默认消息）
    pub fn success_no_data() -> Self {
        Self {
            code: 200,
            msg: "操作成功".into(),
            data: None,
        }
    }

    /// 成功响应（无数据，自定义消息）
    pub fn success_no_data_with_msg(msg: impl Into<String>) -> Self {
        Self {
            code: 200,
            msg: msg.into(),
            data: None,
        }
    }

    /// 失败响应
    pub fn fail(code: i32, msg: String) -> Self {
        Self {
            code,
            msg,
            data: None,
        }
    }
}

/// 分页 API 响应结构体
///
/// 序列化为统一的分页响应格式：
/// ```json
/// {"code": 200, "msg": "查询成功", "rows": [...], "total": 100}
/// ```
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiPageResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    pub rows: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl<T: Serialize> ApiPageResponse<T> {
    /// 从分页数据构造（自定义消息）
    pub fn new(rows: Vec<T>, total: u64, msg: impl Into<String>) -> Self {
        Self {
            code: 200,
            msg: msg.into(),
            rows,
            total: Some(total),
        }
    }

    /// 从分页数据构造（默认消息"查询成功"）
    pub fn page(rows: Vec<T>, total: u64) -> Self {
        Self::new(rows, total, "查询成功")
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let is_prod = std::env::var("APP_ENV")
            .map(|env| matches!(env.as_str(), "prod" | "production"))
            .unwrap_or(false);

        let (status, code, msg, retry_after) = match &self {
            AppError::Validation(s) => (StatusCode::BAD_REQUEST, 400, s.clone(), None),
            AppError::Authentication(s) => (StatusCode::UNAUTHORIZED, 401, s.clone(), None),
            AppError::Authorization(s) => (StatusCode::FORBIDDEN, 403, s.clone(), None),
            AppError::NotFound(s) => (StatusCode::NOT_FOUND, 404, s.clone(), None),
            AppError::Conflict(s) => (StatusCode::CONFLICT, 409, s.clone(), None),
            AppError::PayloadTooLarge(s) => (StatusCode::PAYLOAD_TOO_LARGE, 413, s.clone(), None),
            AppError::RateLimited(s, retry_after) => (
                StatusCode::TOO_MANY_REQUESTS,
                429,
                s.clone(),
                Some(*retry_after),
            ),
            AppError::Database(s) => {
                tracing::error!(error = %s, "database error");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    503,
                    "数据库服务暂不可用".to_string(),
                    None,
                )
            }
            AppError::Config(s) => {
                tracing::error!(error = %s, "config error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    500,
                    internal_error_message(is_prod, s),
                    None,
                )
            }
            AppError::Internal(s) => {
                tracing::error!(error = %s, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    500,
                    internal_error_message(is_prod, s),
                    None,
                )
            }
            AppError::ServiceUnavailable(s) => {
                tracing::error!(error = %s, "service unavailable");
                (StatusCode::SERVICE_UNAVAILABLE, 503, s.clone(), None)
            }
        };
        let body = ApiResponse::<()>::fail(code, msg);
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| r#"{"code":500,"msg":"序列化错误响应失败"}"#.into());

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

fn internal_error_message(is_prod: bool, detail: &str) -> String {
    if is_prod {
        "服务器内部错误".to_string()
    } else {
        detail.to_string()
    }
}

impl From<ValidationErrors> for AppError {
    fn from(e: ValidationErrors) -> Self {
        AppError::Validation(e.to_string())
    }
}
