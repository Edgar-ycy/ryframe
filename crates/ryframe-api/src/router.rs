use std::sync::Arc;

use crate::{
    handlers::{
        auth_handler, authorization_diagnostic_handler, captcha_handler, common_handler,
        config_handler, dept_handler, dict_handler, export_handler, generator_handler, job_handler,
        login_log_handler, menu_handler, message_handler, notice_handler, online_user_handler,
        oper_log_handler, overview_handler, permission_handler, post_handler, profile_handler,
        retention_handler, role_handler, schedule_handler, service_account_handler,
        service_delegation_profile_handler, tenant_config_handler, user_handler,
        user_import_handler,
    },
    oper_log_middleware::{AuditMode, OperLogMiddlewareState, oper_log_middleware},
    request_locale::request_locale_middleware,
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Extension, Request, State},
    http::{HeaderValue, StatusCode, header, header::RETRY_AFTER},
    middleware,
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{delete as delete_route, get as get_route, post},
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_config::RedisMode;
use ryframe_http::{API_PREFIX, ApiResponse, HttpAppError, HttpResult, api_path};
use ryframe_kernel::AppError;
use ryframe_macro::{get, route};
use ryframe_middleware::{
    idempotency::{IdempotencyState, idempotency_middleware},
    metrics::{record_rate_limit_rejection, record_redis_degraded},
    rate_limit::{RateLimitState, user_rate_limit_middleware},
};
use ryframe_service::system::OnlineUserService;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone)]
struct AuthenticatedTenantRateLimitState {
    limiter: Arc<ryframe_middleware::RateLimiter>,
    config: Arc<ryframe_config::RateLimitConfig>,
}

async fn authenticated_tenant_rate_limit(
    State(state): State<AuthenticatedTenantRateLimitState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    if !state.config.enabled {
        return Ok(next.run(request).await);
    }

    let principal = request
        .extensions()
        .get::<RequestPrincipal>()
        .ok_or_else(|| {
            HttpAppError::from(AppError::Authentication("未认证，请先登录".into())).into_response()
        })?;
    if principal.tenant_request_limit_per_minute == 0 {
        return Ok(next.run(request).await);
    }
    let key = ryframe_middleware::RateLimiter::tenant_key(&principal.tenant_id);
    let limit = principal.tenant_request_limit_per_minute;

    match state.limiter.acquire(&key, 60, limit).await {
        Ok(decision) if decision.allowed => Ok(next.run(request).await),
        Ok(decision) => {
            record_rate_limit_rejection("tenant");
            let mut response =
                (StatusCode::TOO_MANY_REQUESTS, "租户请求频率超过配额").into_response();
            if let Ok(value) = HeaderValue::from_str(&decision.retry_after_secs.to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
            Err(response)
        }
        Err(error) => {
            record_redis_degraded("tenant_rate_limit");
            tracing::error!(error = %error, "tenant rate-limit backend unavailable");
            Err((StatusCode::SERVICE_UNAVAILABLE, "限流服务暂不可用").into_response())
        }
    }
}

fn protect<S>(router: Router<S>, state: &AppState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(middleware::from_fn(request_locale_middleware))
        .layer(from_fn_with_state(
            state.services.online_user.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            AuthenticatedTenantRateLimitState {
                limiter: state.rate_limiter.clone(),
                config: Arc::new(state.config.rate_limit.clone()),
            },
            authenticated_tenant_rate_limit,
        ))
        .layer(from_fn_with_state(
            state.auth.clone(),
            ryframe_auth::middleware::auth_middleware,
        ))
}

async fn auth_no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 在线用户跟踪中间件
///
/// 在 auth_middleware 之后运行（RequestPrincipal 和 Claims 已在 extensions 中）。
/// 更新在线索引的最后访问时间。在线索引不是会话授权来源；若索引丢失，
/// 只会影响展示，不会根据 access token 重建一个缺少绝对期限的条目。
async fn online_user_tracking(
    State(online_user_service): State<Arc<OnlineUserService>>,
    request: Request,
    next: Next,
) -> Response {
    // 主体提供已验证身份，Claims 仅提供当前 access token 的唯一标识。
    if let (Some(principal), Some(claims)) = (
        request.extensions().get::<RequestPrincipal>(),
        request.extensions().get::<Claims>(),
    ) {
        online_user_service
            .touch_user(&principal.tenant_id, &claims.sid)
            .await;
    }
    next.run(request).await
}

/// 认证路由
///
/// 路由结构：
/// - 公开路由（无需认证）：/login、/refresh
/// - 受保护路由（auth → oper_log）：/logout、/me
/// - 验证码路由（无需认证）：/captcha/generate、/captcha/verify
/// - 个人资料路由（auth → oper_log）：/profile、/profile/password、/profile/avatar
///
/// 中间件执行顺序（从外到内，先注册的最内层、后注册的最外层先执行）：
///   `public`：操作日志 → 处理器
///   `protected`：认证 → 操作日志 → 处理器
///   `profile`：认证 → 操作日志 → 处理器
pub fn auth_router(state: AppState) -> Router {
    let oper_log_state = OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone());

    // 认证端点可能携带 Cookie、CSRF challenge 或令牌数据，因此绝不进入通用的
    // 操作日志中间件。
    let public = Router::new()
        .route("/csrf", get_route(auth_handler::csrf))
        .route("/login", post(auth_handler::login))
        .route("/refresh", post(auth_handler::refresh))
        .route("/logout", post(auth_handler::logout))
        .route(
            "/password-reset/complete",
            post(auth_handler::complete_password_reset),
        );

    // 当前用户信息是只读技术端点，显式跳过通用操作审计。
    let skipped_protected = Router::new()
        .route("/me", get_route(auth_handler::me))
        .route("/sessions", get_route(auth_handler::list_sessions))
        .layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        ))
        .layer(Extension(AuditMode::Skip));

    // WebSocket ticket 只写入 Redis，显式允许审计事件使用独立 Outbox 事务。
    // 后注册的 Extension 位于操作审计中间件外层，确保策略在中间件执行前可见。
    let independent_protected = Router::new()
        .route("/ws-ticket", post(auth_handler::websocket_ticket))
        .route(
            "/sessions/{sid}",
            delete_route(auth_handler::revoke_session),
        )
        .route(
            "/sessions/revoke-others",
            post(auth_handler::revoke_other_sessions),
        )
        .layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        ))
        .layer(Extension(AuditMode::Independent));

    // protect() 位于最外层，执行顺序为 auth → 审计策略 → oper_log → handler。
    let protected = protect(skipped_protected.merge(independent_protected), &state);

    // 个人资料路由（认证 + 操作日志，中间件在此统一注册）
    // profile_router 不再内嵌 .with_state()
    let profile = protect(
        Router::new()
            .merge(profile_handler::profile_router())
            .layer(from_fn_with_state(oper_log_state, oper_log_middleware)),
        &state,
    );

    Router::new()
        .merge(public)
        .merge(protected)
        .nest("/captcha", captcha_handler::captcha_router())
        .nest("/profile", profile)
        .layer(middleware::from_fn(auth_no_store))
        .with_state(state)
}

/// API 主要入口。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiVersionEndpoints {
    pub auth: String,
    pub system: String,
    pub monitor: String,
    pub tools: String,
    pub common: String,
    pub openapi: String,
    pub swagger: String,
}

/// API 版本与构建信息。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiVersionInfo {
    pub name: String,
    pub version: String,
    pub source_commit: String,
    pub api_prefix: String,
    /// 是否允许客户端选择和管理多个租户。
    pub multi_tenancy_enabled: bool,
    /// 服务账号与个人服务委托功能是否启用。
    pub service_accounts_enabled: bool,
    pub endpoints: ApiVersionEndpoints,
}

/// 返回 API 版本与主要入口。
#[utoipa::path(
    get,
    path = "/api/v1/version",
    tag = "通用",
    responses((status = 200, description = "API 版本与构建信息", body = ApiResponse<ApiVersionInfo>))
)]
pub async fn api_version(State(state): State<AppState>) -> Response {
    let mut response = Json(ApiResponse::success(ApiVersionInfo {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        source_commit: env!("RYFRAME_BUILD_COMMIT").to_owned(),
        api_prefix: API_PREFIX.to_owned(),
        multi_tenancy_enabled: state.config.multi_tenancy.enabled,
        service_accounts_enabled: state.services.service_accounts.is_some(),
        endpoints: ApiVersionEndpoints {
            auth: api_path("auth"),
            system: api_path("system"),
            monitor: api_path("monitor"),
            tools: api_path("tools"),
            common: api_path("common"),
            openapi: api_path("api-docs/openapi.json"),
            swagger: api_path("swagger-ui"),
        },
    }))
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// API 总路由
///
/// `rate_limit_state` 传递到子路由以启用用户级限流。
pub fn api_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let idempotency_state = IdempotencyState::new(state.redis.clone(), 300);
    idempotency_state.spawn_gc();
    let public_runtime = Router::new()
        .route("/ws", get_route(crate::message_socket::upgrade))
        .route("/version", get_route(api_version))
        .with_state(state.clone());
    let mut router = Router::new()
        .merge(public_runtime)
        .nest("/auth", auth_router(state.clone()))
        .nest(
            "/system",
            system_router(
                state.clone(),
                rate_limit_state.clone(),
                idempotency_state.clone(),
            ),
        )
        .nest(
            "/monitor",
            monitor_router(state.clone(), state.monitor.clone()),
        )
        .nest(
            "/tools",
            tools_router(state.clone(), rate_limit_state.clone()),
        )
        .nest("/common", common_router(state.clone()));

    if state.config.multi_tenancy.enabled {
        let platform = protect(
            crate::handlers::tenant_handler::tenant_router(state.clone())
                .layer(from_fn_with_state(
                    OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
                    oper_log_middleware,
                ))
                .layer(from_fn_with_state(
                    idempotency_state.clone(),
                    idempotency_middleware,
                )),
            &state,
        );
        router = router.nest("/platform/tenants", platform);
    }

    if state.services.service_accounts.is_some() {
        let profile_delegations = protect(
            service_delegation_profile_handler::service_delegation_profile_router(state.clone())
                .layer(from_fn_with_state(
                    OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
                    oper_log_middleware,
                )),
            &state,
        );
        router = router.nest("/profile/service-delegations", profile_delegations);
    } else {
        router = router.nest(
            "/profile/service-delegations",
            feature_disabled_router(state.clone()),
        );
    }
    if state.config.api_docs.enabled {
        router = router.route(
            "/api-docs/openapi.json",
            get_route(crate::openapi::openapi_json),
        );
        #[cfg(feature = "runtime-swagger-ui")]
        {
            router = router.merge(swagger_ui_router());
        }
    }
    router.layer(middleware::from_fn(request_locale_middleware))
}

mod groups;
#[path = "router/runtime_status.rs"]
mod runtime_probe;
#[cfg(feature = "runtime-swagger-ui")]
mod swagger;

use groups::{common_router, monitor_router, system_router, tools_router};
use runtime_probe::RuntimeStatus;
#[cfg(feature = "runtime-swagger-ui")]
use swagger::swagger_ui_router;

/// 功能开关关闭时挂载的统一降级路由：返回 501 与稳定的 `feature_disabled` 错误键。
///
/// 前端通过 `/api/v1/version` 的能力字段提前隐藏入口，这里作为纵深防御，
/// 避免功能关闭时把请求落入通用的 404 而无法区分“资源不存在”与“功能未启用”。
pub(super) async fn feature_disabled(State(_state): State<AppState>) -> Response {
    let mut response =
        HttpAppError::from(AppError::FeatureDisabled("服务账号功能未启用".into())).into_response();
    ryframe_http::mark_expected_feature_disabled(&mut response);
    response
}

pub(super) fn feature_disabled_router(state: AppState) -> Router {
    // 使用显式通配路由而不是 fallback：嵌套路由的 fallback 在部分合并/挂载
    // 路径下会被丢弃，通配路由保证功能关闭时任何子路径都返回 501。
    Router::new()
        .route("/", axum::routing::any(feature_disabled))
        .route("/{*rest}", axum::routing::any(feature_disabled))
        .with_state(state)
}

#[get("/runtime")]
#[perm("monitor:runtime:list")]
#[utoipa::path(get, path = "/api/v1/monitor/runtime", tag = "服务器监控",
    responses((status = 200, description = "主应用运行时组件状态", body = ApiResponse<RuntimeStatus>)),
    security(("bearer" = [])))]
pub async fn runtime_status(
    State(state): State<AppState>,
) -> HttpResult<Json<ApiResponse<RuntimeStatus>>> {
    runtime_probe::probe_runtime_status(State(state)).await
}
