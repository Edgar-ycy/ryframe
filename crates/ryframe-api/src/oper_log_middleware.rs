//! 操作日志自动记录中间件
//!
//! 拦截 POST/PUT/DELETE 请求，自动记录操作日志到数据库。
//! 在 auth_middleware 之后运行（RequestPrincipal 已在 extensions 中）。

use std::{sync::Arc, time::Instant};

use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::utils::ip::ClientIp;
use ryframe_service::system::{OperLogService, OperLogStatus, RecordOperLogCommand};

/// 操作日志中间件状态
#[derive(Clone)]
pub struct OperLogMiddlewareState {
    service: Arc<OperLogService>,
}

impl OperLogMiddlewareState {
    /// 创建 Arc 包装的状态（用于 axum layer 注入）
    pub fn new_arc(service: Arc<OperLogService>) -> Arc<Self> {
        Arc::new(Self { service })
    }
}

/// 操作日志中间件
///
/// 对 POST/PUT/DELETE 请求自动记录操作日志。
/// 需要在 auth_middleware 之后运行（RequestPrincipal 已在 extensions 中）。
pub async fn oper_log_middleware(
    State(state): State<Arc<OperLogMiddlewareState>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();

    // 仅对写操作记录日志
    let should_log = matches!(
        method,
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::PATCH
            | axum::http::Method::DELETE
    );

    if !should_log {
        return next.run(request).await;
    }

    let uri = request.extensions().get::<MatchedPath>().map_or_else(
        || request.uri().path().to_string(),
        |path| path.as_str().to_string(),
    );
    let request_method = method.to_string();

    let Some(current_user) = request.extensions().get::<RequestPrincipal>().cloned() else {
        return next.run(request).await;
    };
    let oper_name = current_user.username.clone();

    let oper_ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|client| client.0.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // 推导业务类型和模块标题（基于 URI + HTTP 方法精确映射）
    let (title, business_type) = infer_business_info(&uri, &request_method);

    let start = Instant::now();
    let response = next.run(request).await;
    let cost_time = start.elapsed().as_millis() as i64;

    let http_status = response.status();
    let is_success = http_status.is_success();

    // Never persist request or response bodies in operation logs. A deny-list
    // cannot prove that future DTO fields, configuration values or tokens are
    // safe, and buffering responses also breaks large/streaming downloads.
    let oper_param = None;
    let json_result = None;
    let error_msg = (!is_success).then(|| format!("HTTP {}", http_status.as_u16()));

    let status = if is_success {
        OperLogStatus::Success
    } else {
        OperLogStatus::Failure
    };

    // 异步记录日志（spawn 独立任务，不阻塞响应）
    let command = RecordOperLogCommand {
        title,
        business_type,
        method: format!("{} {}", request_method, uri),
        request_method,
        oper_name,
        oper_url: uri,
        oper_ip,
        oper_param,
        json_result,
        status,
        error_msg,
        cost_time,
    };

    let service = state.service.clone();
    tokio::spawn(async move {
        if let Err(e) = service.record(&current_user, command).await {
            tracing::warn!("操作日志记录失败: {}", e);
        }
    });

    response
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
