use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
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
    #[error("数据库错误: {0}")]
    Database(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("内部错误: {0}")]
    Internal(String),
}
/// 统一 API 响应结构体
///
/// 成功响应：{"code": 200, "message": "操作成功", "data": {...}}
/// 错误响应：{"code": 400, "message": "参数校验失败: xxx", "data": null}
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    /// 成功响应
    pub fn success(data: T) -> Self {
        Self {
            code: 200,
            message: "操作成功".into(),
            data: Some(data),
        }
    }

    /// 成功响应（无数据）
    pub fn success_no_data() -> ApiResponse<()> {
        ApiResponse {
            code: 200,
            message: "操作成功".into(),
            data: None,
        }
    }

    /// 失败响应
    pub fn fail(code: i32, message: String) -> ApiResponse<()> {
        ApiResponse {
            code,
            message,
            data: None,
        }
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::Validation(msg) => (StatusCode::BAD_REQUEST, 400, msg.clone()),
            AppError::Authentication(msg) => (StatusCode::UNAUTHORIZED, 401, msg.clone()),
            AppError::Authorization(msg) => (StatusCode::FORBIDDEN, 403, msg.clone()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, 404, msg.clone()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, 409, msg.clone()),
            AppError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 500, msg.clone()),
            AppError::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 500, msg.clone()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 500, msg.clone()),
        };
        let body = ApiResponse::<()>::fail(code, message);
        let json = serde_json::to_string(&body).unwrap_or_else(|_| {
            r#"{"code":500,"message":"序列化错误响应失败","data":null}"#.into()
        });

        let mut response = Response::new(axum::body::Body::from(json));
        *response.status_mut() = status;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        response
    }
}
