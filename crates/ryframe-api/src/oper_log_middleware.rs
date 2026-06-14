//! 操作日志自动记录中间件
//!
//! 拦截 POST/PUT/DELETE 请求，自动记录操作日志到数据库。
//! 在 auth_middleware 之后运行（Claims 已在 extensions 中）。

use std::{sync::Arc, time::Instant};

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http_body_util::BodyExt;
use ryframe_common::utils::snowflake;
use ryframe_core::Repository;
use ryframe_db::{OperLogRepository, entities::oper_log};
use sea_orm::DatabaseConnection;

/// 操作日志中间件状态
#[derive(Clone)]
pub struct OperLogMiddlewareState {
    pub db: DatabaseConnection,
}

impl OperLogMiddlewareState {
    /// 创建 Arc 包装的状态（用于 axum layer 注入）
    pub fn new_arc(db: DatabaseConnection) -> Arc<Self> {
        Arc::new(Self { db })
    }
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

    // 提取操作者信息（从 Claims）
    let oper_name = request
        .extensions()
        .get::<ryframe_auth::jwt::Claims>()
        .map(|c| c.username.clone())
        .unwrap_or_else(|| "anonymous".to_string());

    // 提取客户端 IP（支持反向代理逗号分隔场景）
    let oper_ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // 推导业务类型和模块标题（基于 URI + HTTP 方法精确映射）
    let (title, business_type) = infer_business_info(&uri, &request_method);

    // 检查是否为文件上传请求（multipart/form-data 的 body 被消费后无法被 Multipart 解析）
    let is_multipart = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("multipart/form-data"))
        .unwrap_or(false);

    // 缓存请求体（用于记录操作参数）—— multipart 请求跳过 body 消费
    let (parts, body) = request.into_parts();
    let (request, oper_param) = if is_multipart {
        // multipart 请求不消费 body，直接透传原始流
        let req = Request::from_parts(parts, body);
        (req, Some("[文件上传]".to_string()))
    } else {
        let bytes = body
            .collect()
            .await
            .map(|c| c.to_bytes())
            .unwrap_or_default();
        let param = if bytes.is_empty() {
            None
        } else {
            let s = String::from_utf8_lossy(&bytes);
            let truncated = if s.len() > 2000 {
                format!("{}...[truncated]", truncate_str(&s, 2000))
            } else {
                s.to_string()
            };
            Some(truncated)
        };
        let req = Request::from_parts(parts, Body::from(bytes));
        (req, param)
    };

    let start = Instant::now();
    let response = next.run(request).await;
    let cost_time = start.elapsed().as_millis() as i64;

    let http_status = response.status();
    let is_success = http_status.is_success();

    // 提取响应体（截断过长内容）
    let (response_parts, response_body) = response.into_parts();
    let response_bytes = response_body
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();

    let json_result = if response_bytes.is_empty() {
        None
    } else {
        let s = String::from_utf8_lossy(&response_bytes);
        let truncated = if s.len() > 2000 {
            format!("{}...[truncated]", truncate_str(&s, 2000))
        } else {
            s.to_string()
        };
        Some(truncated)
    };

    // 提取错误消息（仅失败时）
    let error_msg = if !is_success {
        extract_error_message(&json_result)
    } else {
        None
    };

    let log_status = if is_success {
        oper_log::Model::STATUS_SUCCESS
    } else {
        oper_log::Model::STATUS_FAIL
    };

    // 异步记录日志（spawn 独立任务，不阻塞响应）
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
        json_result,
        status: log_status.to_string(),
        error_msg,
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

    // 重建响应
    Response::from_parts(response_parts, Body::from(response_bytes))
}

/// 从 JSON 响应体中提取错误消息
fn extract_error_message(json_result: &Option<String>) -> Option<String> {
    json_result.as_ref().and_then(|s| {
        serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str().map(String::from)))
    })
}

/// 安全截断字符串（按字符边界截断，避免 UTF-8 字节边界 panic）
fn truncate_str(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// 根据 URI 路径 + HTTP 方法推导业务类型和模块标题
///
/// 路径格式: /api/v1/{module}/{resource}[/{sub}]
/// 返回: (模块中文标题, 业务类型)
fn infer_business_info(uri: &str, method: &str) -> (String, String) {
    let segments: Vec<&str> = uri.split('/').filter(|s| !s.is_empty()).collect();

    // 模块名从路径第3段提取: /api/v1/system/users → "system"
    let module = segments.get(2).copied().unwrap_or("unknown");
    // 资源名从路径第4段提取: /api/v1/system/users → "users"
    let resource = segments.get(3).copied().unwrap_or("unknown");

    // 映射 resource → 中文标题
    let title = resource_to_title(module, resource);

    // 根据 HTTP 方法 + URI 细化业务类型
    let business_type = match method {
        "POST" => {
            if uri.ends_with("/import") {
                "IMPORT"
            } else if uri.contains("upload") {
                "UPLOAD"
            } else {
                "INSERT"
            }
        }
        "PUT" => "UPDATE",
        "DELETE" => {
            if uri.contains("/clean") {
                "CLEAN"
            } else if uri.contains("/online") {
                "FORCE_LOGOUT"
            } else {
                "DELETE"
            }
        }
        _ => "OTHER",
    }
    .to_string();

    (title, business_type)
}

/// 将 (module, resource) 映射为中文模块标题
fn resource_to_title(module: &str, resource: &str) -> String {
    match (module, resource) {
        ("auth", "login") => "用户登录".into(),
        ("auth", "logout") => "用户登出".into(),
        ("auth", "captcha") => "验证码".into(),
        ("auth", "profile") => "个人中心".into(),
        ("system", "users") => "用户管理".into(),
        ("system", "roles") => "角色管理".into(),
        ("system", "permissions") => "权限管理".into(),
        ("system", "menus") => "菜单管理".into(),
        ("system", "depts") => "部门管理".into(),
        ("system", "posts") => "岗位管理".into(),
        ("system", "configs") => "参数配置".into(),
        ("system", "dict") => "字典管理".into(),
        ("system", "notices") => "通知公告".into(),
        ("system", "operlogs") => "操作日志".into(),
        ("system", "loginlogs") => "登录日志".into(),
        ("system", "online") => "在线用户".into(),
        ("monitor", _) => "服务监控".into(),
        ("tools", "gen") => "代码生成".into(),
        ("common", _) => "通用功能".into(),
        _ => {
            // 兜底：资源名首字母大写
            let mut chars = resource.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => "未知模块".into(),
            }
        }
    }
}
