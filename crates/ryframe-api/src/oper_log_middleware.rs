//! 操作日志自动记录中间件
//!
//! 拦截 POST/PUT/DELETE 请求，自动记录操作日志到数据库。

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http_body_util::BodyExt;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;
use ryframe_db::OperLogRepository;
use ryframe_db::entities::oper_log;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use std::time::Instant;

/// 操作日志中间件状态
#[derive(Clone)]
pub struct OperLogMiddlewareState {
    pub db: DatabaseConnection,
}

/// 操作日志中间件
///
/// 对 POST/PUT/DELETE 请求自动记录操作日志。
/// 需要在 auth_middleware 之后运行（Claims 已在 extensions 中）。
pub async fn oper_log_middleware(
    State(state): State<Arc<OperLogMiddlewareState>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();

    // 仅对写操作记录日志
    let should_log = matches!(
        method,
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::DELETE
    );

    if !should_log {
        return next.run(request).await;
    }

    let uri = request.uri().path().to_string();
    let request_method = method.to_string();

    // 提取操作者信息（从 Claims，优先使用 username）
    let oper_name = request
        .extensions()
        .get::<ryframe_auth::jwt::Claims>()
        .map(|c| c.username.clone())
        .unwrap_or_else(|| "anonymous".to_string());

    // 提取 IP
    let oper_ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    // 推导业务类型和模块标题
    let (title, business_type) = infer_business_info(&uri, &request_method);

    // 缓存请求体（用于记录操作参数）
    let (parts, body) = request.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();
    let oper_param = if body_bytes.is_empty() {
        None
    } else {
        let s = String::from_utf8_lossy(&body_bytes);
        // 截断过长的请求体
        let truncated = if s.len() > 2000 {
            format!("{}...", &s[..2000])
        } else {
            s.to_string()
        };
        Some(truncated)
    };

    // 重建请求
    let request = Request::from_parts(parts, Body::from(body_bytes));

    let start = Instant::now();
    let response = next.run(request).await;
    let cost_time = start.elapsed().as_millis() as i64;

    let status = if response.status().is_success() {
        oper_log::Model::STATUS_SUCCESS
    } else {
        oper_log::Model::STATUS_FAIL
    };

    // 异步记录日志（不阻塞响应）
    let log_entry = oper_log::Model {
        id: snowflake::next_snowflake_id(),
        title,
        business_type,
        method: format!("{} {}", request_method, uri),
        request_method,
        oper_name,
        oper_url: uri,
        oper_ip,
        oper_location: None,
        oper_param,
        json_result: None,
        status: status.to_string(),
        error_msg: None,
        oper_time: chrono::Utc::now(),
        cost_time,
    };

    let db = state.db.clone();
    let repo = OperLogRepository;
    tokio::spawn(async move {
        if let Err(e) = repo.insert(&db, log_entry).await {
            tracing::warn!("操作日志记录失败: {}", e);
        }
    });

    response
}

/// 根据 URI 和 HTTP 方法推导业务类型和模块标题
fn infer_business_info(uri: &str, method: &str) -> (String, String) {
    let business_type = match method {
        "POST" => "INSERT",
        "PUT" => "UPDATE",
        "DELETE" => "DELETE",
        _ => "OTHER",
    }
    .to_string();

    // 从 URI 路径提取模块名: /api/v1/system/users/xxx → "users"
    let segments: Vec<&str> = uri.split('/').filter(|s| !s.is_empty()).collect();
    let title = if segments.len() >= 3 {
        segments[2].to_string()
    } else if segments.len() >= 2 {
        segments[1].to_string()
    } else {
        "unknown".to_string()
    };

    (title, business_type)
}
