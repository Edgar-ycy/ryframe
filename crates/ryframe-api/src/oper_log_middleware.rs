//! 操作审计自动记录中间件。
//!
//! 拦截写请求并同步持久化事务 Outbox 意图，实际日志由 Worker 幂等落库。
//! 在认证中间件之后运行，此时 `RequestPrincipal` 已在扩展中。

use std::sync::Arc;

use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_auth::RequestPrincipal;
use ryframe_middleware::request_id::RequestId;
use ryframe_service::{
    AuditOutbox, AuditRequestContext, scope_audit_request,
    system::{OperLogStatus, RecordOperLogCommand},
};
use ryframe_utils::ip::ClientIp;
use uuid::Uuid;

/// 写请求使用的操作审计事务策略。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AuditMode {
    /// 数据库业务写入必须在同一事务中提交审计 Outbox。
    #[default]
    Transactional,
    /// Redis、对象存储或外部系统写入允许使用独立短事务记录审计。
    Independent,
    /// 只读或纯技术端点不生成通用操作审计。
    Skip,
}

impl AuditMode {
    fn requires_transaction(self) -> bool {
        matches!(self, Self::Transactional)
    }
}

/// 操作日志中间件状态
#[derive(Clone)]
pub struct OperLogMiddlewareState {
    outbox: Arc<AuditOutbox>,
}

impl OperLogMiddlewareState {
    /// 创建供 axum 中间件注入的共享状态。
    pub fn new_arc(outbox: Arc<AuditOutbox>) -> Arc<Self> {
        Arc::new(Self { outbox })
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
    let audit_mode = request
        .extensions()
        .get::<AuditMode>()
        .copied()
        .unwrap_or_default();
    if matches!(audit_mode, AuditMode::Skip) {
        return next.run(request).await;
    }

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

    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let context = match AuditRequestContext::new(
        Uuid::now_v7().to_string(),
        request_id,
        current_user.tenant_id.clone(),
        RecordOperLogCommand {
            title,
            business_type,
            method: format!("{} {}", request_method, uri),
            request_method,
            oper_name,
            oper_url: uri,
            oper_ip,
            oper_param: None,
            json_result: None,
            status: OperLogStatus::Success,
            error_msg: None,
            cost_time: 0,
        },
    ) {
        Ok(context) => context,
        Err(error) => {
            ryframe_service::record_audit_failure("context");
            tracing::error!(%error, "无法创建操作审计上下文");
            return next.run(request).await;
        }
    };

    let response = scope_audit_request(context.clone(), next.run(request)).await;

    let http_status = response.status();
    let is_success = http_status.is_success();

    let error_msg = (!is_success).then(|| format!("HTTP {}", http_status.as_u16()));

    let status = if is_success {
        OperLogStatus::Success
    } else {
        OperLogStatus::Failure
    };

    // 业务事务已经原子提交 Outbox 时无需重复写入；其他路径使用独立短事务。
    // 审计故障只记录指标与错误日志，绝不覆盖原始业务响应。
    if !(is_success && context.transaction_committed()) {
        if is_success && audit_mode.requires_transaction() && !context.transaction_bound() {
            ryframe_service::record_audit_failure("transaction_unbound");
            tracing::warn!("事务型写请求尚未接入业务事务审计绑定，使用独立 Outbox 事务");
        }
        let event = context.event(status, error_msg);
        if let Err(error) = state.outbox.record(&event).await {
            ryframe_service::record_audit_failure("outbox_record");
            tracing::error!(
                error = %error,
                event_id = %event.event_id,
                request_id = %event.request_id,
                audit_mode = ?audit_mode,
                "操作审计 Outbox 持久化失败"
            );
        }
    }

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
            if uri.ends_with("/sessions/revoke-others") {
                "FORCE_LOGOUT"
            } else if uri.ends_with("/import") {
                "IMPORT"
            } else if uri.contains("upload") {
                "UPLOAD"
            } else {
                "INSERT"
            }
        }
        "PUT" => "UPDATE",
        "DELETE" => {
            if uri.contains("/auth/sessions/") {
                "FORCE_LOGOUT"
            } else if uri.contains("/clean") {
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
        ("auth", "sessions") => "登录设备".into(),
        ("system", "users") => "用户管理".into(),
        ("system", "roles") => "角色管理".into(),
        ("system", "permissions") => "权限管理".into(),
        ("system", "menus") => "菜单管理".into(),
        ("system", "depts") => "部门管理".into(),
        ("system", "posts") => "岗位管理".into(),
        ("system", "configs") => "参数配置".into(),
        ("system", "config-packages" | "config-transfers") => "配置迁移".into(),
        ("system", "service-accounts") => "服务账号".into(),
        ("system", "service-delegations") => "服务委托".into(),
        ("system", "service-access-audits") => "服务访问审计".into(),
        ("profile", "service-delegations") => "个人服务委托".into(),
        ("system", "dict") => "字典管理".into(),
        ("system", "notices") => "通知公告".into(),
        ("system", "operlogs") => "操作日志".into(),
        ("system", "loginlogs") => "登录日志".into(),
        ("system", "online") => "在线用户".into(),
        ("platform", "tenants") => "租户管理".into(),
        ("monitor", "jobs") => "后台任务".into(),
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
