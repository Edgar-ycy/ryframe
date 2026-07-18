use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header, header::RETRY_AFTER},
    middleware,
    middleware::{Next, from_fn_with_state},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_config::RedisMode;
use ryframe_macro::{get, route};
use ryframe_middleware::{
    idempotency::{IdempotencyState, idempotency_middleware},
    metrics::{record_rate_limit_rejection, record_redis_degraded},
    rate_limit::{RateLimitState, user_rate_limit_middleware},
};
use ryframe_service::system::OnlineUserService;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::{
    handlers::{
        auth_handler, captcha_handler, common_handler, config_handler, dept_handler, dict_handler,
        generator_handler, login_log_handler, menu_handler, notice_handler, online_user_handler,
        oper_log_handler, permission_handler, post_handler, profile_handler, role_handler,
        user_handler,
    },
    oper_log_middleware::{OperLogMiddlewareState, oper_log_middleware},
    state::AppState,
};

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
        .ok_or_else(|| AppError::Authentication("未认证，请先登录".into()).into_response())?;
    let key = format!("tenant:{}", principal.tenant_id);
    let limit = principal.tenant_request_limit_per_minute.max(1);

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
        .layer(from_fn_with_state(
            AuthenticatedTenantRateLimitState {
                limiter: state.rate_limiter.clone(),
                config: Arc::new(state.config.rate_limit.clone()),
            },
            authenticated_tenant_rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
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
/// - public (no auth): /login, /refresh
/// - protected (auth → oper_log): /logout, /me
/// - captcha (no auth): /captcha/generate, /captcha/verify
/// - profile (auth → oper_log): /profile, /profile/password, /profile/avatar
///
/// 中间件执行顺序（从外到内，先注册的最内层、后注册的最外层先执行）：
///   public:  oper_log → handler
///   protected: auth → oper_log → handler
///   profile:  auth → oper_log → handler
pub fn auth_router(state: AppState) -> Router {
    let oper_log_state = OperLogMiddlewareState::new_arc(state.services.oper_log.clone());

    // Authentication endpoints can carry cookies, CSRF challenges or token data,
    // so they never enter the generic operation-log middleware.
    let public = Router::new()
        .route("/csrf", get(auth_handler::csrf))
        .route("/login", post(auth_handler::login))
        .route("/refresh", post(auth_handler::refresh))
        .route("/logout", post(auth_handler::logout))
        .route(
            "/password-reset/complete",
            post(auth_handler::complete_password_reset),
        );

    // 受保护路由
    // .layer() 从后往前执行：auth（外层先执行）→ oper_log（内层后执行）→ handler
    let protected = protect(
        Router::new()
            .route("/me", get(auth_handler::me))
            .layer(from_fn_with_state(
                oper_log_state.clone(),
                oper_log_middleware,
            )),
        &state,
    );

    // Profile 路由（认证 + 操作日志，中间件在此统一注册）
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

/// API 版本信息
async fn api_version() -> Json<serde_json::Value> {
    Json(json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "api_prefix": "/api/v1",
        "endpoints": {
            "auth": "/api/v1/auth",
            "system": "/api/v1/system",
            "monitor": "/api/v1/monitor",
            "tools": "/api/v1/tools",
            "common": "/api/v1/common",
            "openapi": "/api/v1/api-docs/openapi.json",
            "swagger": "/api/v1/swagger-ui"
        }
    }))
}

/// API 总路由
///
/// `rate_limit_state` 传递到子路由以启用用户级限流。
pub fn api_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let idempotency_state = IdempotencyState::new(state.redis.clone(), 300);
    idempotency_state.spawn_gc();
    let platform = protect(
        crate::handlers::tenant_handler::tenant_router(state.clone()).layer(from_fn_with_state(
            idempotency_state.clone(),
            idempotency_middleware,
        )),
        &state,
    );

    Router::new()
        .nest("/auth", auth_router(state.clone()))
        .nest("/platform/tenants", platform)
        .nest(
            "/system",
            system_router(state.clone(), rate_limit_state.clone(), idempotency_state),
        )
        .nest(
            "/monitor",
            monitor_router(state.clone(), state.monitor.clone()),
        )
        .nest(
            "/tools",
            tools_router(state.clone(), rate_limit_state.clone()),
        )
        .nest("/common", common_router(state.clone()))
        // API 版本信息端点
        .route("/version", get(api_version))
        // OpenAPI JSON 文档: /api-docs/openapi.json
        .route("/api-docs/openapi.json", get(crate::openapi::openapi_json))
        // Swagger UI 交互文档: /swagger-ui
        .route("/swagger-ui", get(swagger_ui))
}

fn monitor_router(state: AppState, monitor_state: ryframe_monitor::MonitorState) -> Router {
    let public = ryframe_monitor::public_monitor_router(monitor_state.clone());
    let protected = ryframe_monitor::protected_monitor_router(monitor_state)
        .merge(route!(runtime_status).with_state(state.clone()));

    public.merge(protect(protected, &state))
}

#[get("/runtime")]
#[perm("monitor:runtime:list")]
#[utoipa::path(get, path = "/api/v1/monitor/runtime", tag = "服务器监控",
    responses((status = 200, description = "主应用运行时组件状态", body = ApiResponse<RuntimeStatus>)),
    security(("bearer" = [])))]
async fn runtime_status(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<RuntimeStatus>>> {
    let database_health = state.monitor.database.topology_health().await;
    let replicas_connected = database_health
        .replicas
        .iter()
        .all(|replica| replica.healthy);
    let replicas = database_health
        .replicas
        .into_iter()
        .map(|replica| RuntimeDatabaseReplicaStatus {
            name: replica.name,
            connected: replica.healthy,
        })
        .collect::<Vec<_>>();
    let sources_connected = database_health.sources.iter().all(|source| source.healthy);
    let sources = database_health
        .sources
        .into_iter()
        .map(|source| RuntimeDatabaseSourceStatus {
            name: source.name,
            connected: source.healthy,
        })
        .collect::<Vec<_>>();
    let read_policy = if replicas.is_empty() {
        "primary"
    } else {
        "round_robin"
    };
    let storage_connected = state.services.file.check_storage().await.is_ok();
    let storage_config = &state.config.object_storage;

    Ok(Json(ApiResponse::success(RuntimeStatus {
        database: RuntimeDatabaseStatus {
            connected: database_health.primary_healthy && replicas_connected && sources_connected,
            driver: "mysql".into(),
            primary_connected: database_health.primary_healthy,
            replica_count: replicas.len(),
            replicas,
            source_count: sources.len(),
            sources,
            read_policy: read_policy.into(),
        },
        redis: RuntimeRedisStatus {
            configured: state
                .config
                .redis
                .as_ref()
                .is_some_and(|config| config.mode != RedisMode::Disabled),
            connected: state.redis.is_some(),
        },
        object_storage: RuntimeStorageStatus {
            backend: storage_config.backend.as_str().into(),
            connected: storage_connected,
            endpoint: (!storage_config.endpoint.trim().is_empty())
                .then(|| storage_config.endpoint.clone()),
        },
        upload_circuit_breaker: RuntimeCircuitBreakerStatus {
            state: format!("{:?}", state.runtime.upload_circuit_breaker.current_state()),
        },
    })))
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeStatus {
    database: RuntimeDatabaseStatus,
    redis: RuntimeRedisStatus,
    object_storage: RuntimeStorageStatus,
    upload_circuit_breaker: RuntimeCircuitBreakerStatus,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseStatus {
    connected: bool,
    driver: String,
    primary_connected: bool,
    replica_count: usize,
    replicas: Vec<RuntimeDatabaseReplicaStatus>,
    source_count: usize,
    sources: Vec<RuntimeDatabaseSourceStatus>,
    read_policy: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseReplicaStatus {
    name: String,
    connected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseSourceStatus {
    name: String,
    connected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeRedisStatus {
    configured: bool,
    connected: bool,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeStorageStatus {
    backend: String,
    connected: bool,
    endpoint: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeCircuitBreakerStatus {
    state: String,
}

/// 系统管理路由（认证主体 + 租户限流 + 用户限流 + 在线跟踪 + 操作日志）
///
/// .layer() 链的语义：后注册的 layer 包裹先注册的，即后注册的先执行（外层先执行）。
/// 执行顺序（从外到内）：
///   1. auth_middleware（一次注入 RequestPrincipal）
///   2. authenticated_tenant_rate_limit（使用已认证租户）
///   3. user_rate_limit_middleware
///   4. online_user_tracking
///   5. oper_log_middleware
fn system_router(
    state: AppState,
    rate_limit_state: RateLimitState,
    idempotency_state: IdempotencyState,
) -> Router {
    let router = Router::new()
        .nest("/users", user_handler::user_router(state.clone()))
        .nest("/roles", role_handler::role_router(state.clone()))
        .nest(
            "/perms",
            permission_handler::permission_router(state.clone()),
        )
        .nest("/menus", menu_handler::menu_router(state.clone()))
        .nest("/depts", dept_handler::dept_router(state.clone()))
        .nest("/posts", post_handler::post_router(state.clone()))
        .nest("/configs", config_handler::config_router(state.clone()))
        .nest("/dict", dict_handler::dict_router(state.clone()))
        .nest("/notices", notice_handler::notice_router(state.clone()))
        .nest(
            "/operlogs",
            oper_log_handler::oper_log_router(state.clone()),
        )
        .nest(
            "/loginlogs",
            login_log_handler::login_log_router(state.clone()),
        )
        .nest(
            "/online",
            online_user_handler::online_user_router(state.clone()),
        )
        // 从内到外注册：内层 layer 先注册
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.oper_log.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            idempotency_state,
            idempotency_middleware,
        ))
        .layer(from_fn_with_state(
            state.services.online_user.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ));

    protect(router, &state)
}

/// 工具路由（认证主体 + 租户限流 + 用户限流 + 操作日志）
///
/// 执行顺序（从外到内）：auth → tenant_rate_limit → user_rate_limit → oper_log
fn tools_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let router = Router::new()
        .nest("/gen", generator_handler::generator_router(state.clone()))
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.oper_log.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ));

    protect(router, &state)
}

/// 通用功能路由（文件上传等）
/// 上传和下载都要求认证主体，并记录操作日志。
fn common_router(state: AppState) -> Router {
    let oper_log_state = OperLogMiddlewareState::new_arc(state.services.oper_log.clone());

    let upload = protect(
        common_handler::upload_router(state.clone()).layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        )),
        &state,
    );

    let download = protect(
        common_handler::download_router(state.clone())
            .layer(from_fn_with_state(oper_log_state, oper_log_middleware)),
        &state,
    );

    Router::new()
        .nest("/upload", upload)
        .nest("/file", download)
}

/// Swagger UI 交互文档页面
///
/// 利用 CDN 加载 Swagger UI，指向本服务的 OpenAPI JSON。
async fn swagger_ui() -> Html<&'static str> {
    Html(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>RyFrame API 文档</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
    <style>
        html { box-sizing: border-box; overflow-y: scroll; }
        *, *:before, *:after { box-sizing: inherit; }
        body { margin: 0; background: #fafafa; }
        .topbar { display: none; }
        .swagger-ui .info .title { font-size: 2em; }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js" crossorigin></script>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js" crossorigin></script>
    <script>
        window.onload = () => {
            window.ui = SwaggerUIBundle({
                url: "/api/v1/api-docs/openapi.json",
                dom_id: "#swagger-ui",
                deepLinking: true,
                presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
                layout: "StandaloneLayout",
                defaultModelsExpandDepth: 1,
                defaultModelExpandDepth: 1,
                docExpansion: "list",
                filter: true,
                showExtensions: true,
                showCommonExtensions: true,
            });
        };
    </script>
</body>
</html>"##,
    )
}
