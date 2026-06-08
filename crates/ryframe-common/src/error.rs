use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
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
    #[error("数据库错误: {0}")]
    Database(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("内部错误: {0}")]
    Internal(String),
}
/// 统一 API 响应结构体
///
/// 成功响应（单对象/列表）：{"code": 200, "msg": "操作成功", "data": {...}}
/// 成功响应（无数据）：{"code": 200, "msg": "操作成功"}
/// 错误响应：{"code": 400, "msg": "参数校验失败: xxx"}
///
/// 分页查询请使用 [`ApiPageResponse`]。
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
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
#[derive(Debug, Serialize)]
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
        let (status, code, msg) = match &self {
            AppError::Validation(s) => (StatusCode::BAD_REQUEST, 400, s.clone()),
            AppError::Authentication(s) => (StatusCode::UNAUTHORIZED, 401, s.clone()),
            AppError::Authorization(s) => (StatusCode::FORBIDDEN, 403, s.clone()),
            AppError::NotFound(s) => (StatusCode::NOT_FOUND, 404, s.clone()),
            AppError::Conflict(s) => (StatusCode::CONFLICT, 409, s.clone()),
            AppError::Database(s) => (StatusCode::INTERNAL_SERVER_ERROR, 500, s.clone()),
            AppError::Config(s) => (StatusCode::INTERNAL_SERVER_ERROR, 500, s.clone()),
            AppError::Internal(s) => (StatusCode::INTERNAL_SERVER_ERROR, 500, s.clone()),
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
        response
    }
}

impl From<ValidationErrors> for AppError {
    fn from(e: ValidationErrors) -> Self {
        AppError::Validation(e.to_string())
    }
}
