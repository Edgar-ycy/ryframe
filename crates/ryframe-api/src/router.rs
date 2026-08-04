use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header, header::RETRY_AFTER},
    middleware,
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get as get_route, post},
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
use utoipa_swagger_ui::{Config as SwaggerUiConfig, serve as serve_swagger_ui};

use crate::{
    handlers::{
        auth_handler, captcha_handler, common_handler, config_handler, dept_handler, dict_handler,
        export_handler, generator_handler, job_handler, login_log_handler, menu_handler,
        message_handler, notice_handler, online_user_handler, oper_log_handler, permission_handler,
        post_handler, profile_handler, role_handler, user_handler,
    },
    oper_log_middleware::{OperLogMiddlewareState, oper_log_middleware},
    request_locale::request_locale_middleware,
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
        .ok_or_else(|| {
            HttpAppError::from(AppError::Authentication("未认证，请先登录".into())).into_response()
        })?;
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
        .layer(middleware::from_fn(request_locale_middleware))
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

    // 受保护路由
    // .layer() 从后往前执行：auth（外层先执行）→ oper_log（内层后执行）→ handler
    let protected = protect(
        Router::new()
            .route("/me", get_route(auth_handler::me))
            .route("/ws-ticket", post(auth_handler::websocket_ticket))
            .layer(from_fn_with_state(
                oper_log_state.clone(),
                oper_log_middleware,
            )),
        &state,
    );

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
    pub endpoints: ApiVersionEndpoints,
}

/// 返回 API 版本与主要入口。
#[utoipa::path(
    get,
    path = "/api/v1/version",
    tag = "通用",
    responses((status = 200, description = "API 版本与构建信息", body = ApiResponse<ApiVersionInfo>))
)]
pub async fn api_version() -> Json<ApiResponse<ApiVersionInfo>> {
    Json(ApiResponse::success(ApiVersionInfo {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        source_commit: env!("RYFRAME_BUILD_COMMIT").to_owned(),
        api_prefix: API_PREFIX.to_owned(),
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
}

/// API 总路由
///
/// `rate_limit_state` 传递到子路由以启用用户级限流。
pub fn api_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let idempotency_state = IdempotencyState::new(state.redis.clone(), 300);
    idempotency_state.spawn_gc();
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

    let mut router = Router::new()
        .route("/ws", get_route(crate::message_socket::upgrade))
        .with_state(state.clone())
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
        .route("/version", get_route(api_version));

    if state.config.api_docs.enabled {
        router = router
            .route(
                "/api-docs/openapi.json",
                get_route(crate::openapi::openapi_json),
            )
            .merge(swagger_ui_router());
    }
    router.layer(middleware::from_fn(request_locale_middleware))
}

fn monitor_router(state: AppState, monitor_state: ryframe_monitor::MonitorState) -> Router {
    let public = ryframe_monitor::public_monitor_router(monitor_state.clone());
    let protected = ryframe_monitor::protected_monitor_router(monitor_state)
        .merge(route!(runtime_status).with_state(state.clone()))
        .merge(job_handler::job_router(state.clone()))
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
            oper_log_middleware,
        ));

    public.merge(protect(protected, &state))
}

#[get("/runtime")]
#[perm("monitor:runtime:list")]
#[utoipa::path(get, path = "/api/v1/monitor/runtime", tag = "服务器监控",
    responses((status = 200, description = "主应用运行时组件状态", body = ApiResponse<RuntimeStatus>)),
    security(("bearer" = [])))]
async fn runtime_status(
    State(state): State<AppState>,
) -> HttpResult<Json<ApiResponse<RuntimeStatus>>> {
    let database_health = state.monitor.database.topology_health().await;
    let replicas_connected = database_health
        .replicas
        .iter()
        .all(|replica| replica.healthy);
    let healthy_replica_count = database_health
        .replicas
        .iter()
        .filter(|replica| replica.healthy)
        .count();
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
    let read_policy = match (replicas.len(), healthy_replica_count) {
        (0, _) => "primary",
        (_, 0) => "primary_fallback",
        _ => "round_robin",
    };
    let storage_connected = state.services.file.check_storage().await.is_ok();
    let storage_config = &state.config.object_storage;
    let read_selections = ryframe_middleware::metrics::database_read_selection_totals()
        .into_iter()
        .map(|(target, reason, count)| RuntimeDatabaseReadSelection {
            target: target.into(),
            reason: reason.into(),
            count,
        })
        .collect();

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
            read_fallback_total: ryframe_middleware::metrics::database_read_fallback_total(),
            read_selections,
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
    read_fallback_total: u64,
    read_selections: Vec<RuntimeDatabaseReadSelection>,
}

#[derive(Debug, Serialize, ToSchema)]
struct RuntimeDatabaseReadSelection {
    target: String,
    reason: String,
    count: u64,
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
///   3. 用户限流中间件（`user_rate_limit_middleware`）
///   4. 在线用户跟踪（`online_user_tracking`）
///   5. 操作日志中间件（`oper_log_middleware`）
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
        .nest("/messages", message_handler::message_router(state.clone()))
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
            OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
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
            OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone()),
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
    let oper_log_state = OperLogMiddlewareState::new_arc(state.services.audit_outbox.clone());

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
    let exports = protect(export_handler::export_router(state.clone()), &state);

    Router::new()
        .nest("/upload", upload)
        .nest("/file", download)
        .nest("/jobs", exports)
}

const SWAGGER_UI_NO_CACHE: &str = "no-store";
const SWAGGER_UI_STATIC_CACHE: &str = "public, max-age=86400";

fn swagger_ui_base_element() -> String {
    format!("<base href=\"{}/swagger-ui/\">", API_PREFIX)
}

fn swagger_ui_router() -> Router {
    Router::new()
        .route("/swagger-ui", get_route(swagger_ui_index))
        .route("/swagger-ui/{*asset}", get_route(swagger_ui_asset))
}

/// 返回唯一的 Swagger UI 文档入口，不提供尾斜杠兼容路由或重定向。
async fn swagger_ui_index() -> Response {
    swagger_ui_response("")
}

/// 返回编译进二进制的 Swagger UI 静态资源。
async fn swagger_ui_asset(Path(asset): Path<String>) -> Response {
    let asset = asset.trim_start_matches('/');
    if asset.is_empty() || asset == "index.html" || asset.contains('/') || asset.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    swagger_ui_response(asset)
}

fn swagger_ui_response(asset: &str) -> Response {
    let config = Arc::new(
        SwaggerUiConfig::from(api_path("api-docs/openapi.json"))
            .deep_linking(true)
            .default_models_expand_depth(1)
            .default_model_expand_depth(1)
            .doc_expansion("list")
            .filter(true)
            .show_extensions(true)
            .show_common_extensions(true)
            .validator_url("none"),
    );

    match serve_swagger_ui(asset, config) {
        Ok(Some(file)) => {
            let bytes = if asset.is_empty() {
                match localize_swagger_index(file.bytes.into_owned()) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::error!(%error, "无法解析内嵌 Swagger UI 首页");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                }
            } else {
                file.bytes.into_owned()
            };
            let cache_control = if asset.is_empty() || asset == "swagger-initializer.js" {
                SWAGGER_UI_NO_CACHE
            } else {
                SWAGGER_UI_STATIC_CACHE
            };

            match Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, file.content_type)
                .header(header::CACHE_CONTROL, cache_control)
                .body(Body::from(bytes))
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::error!(%error, "无法构造内嵌 Swagger UI 响应");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(%error, "无法读取内嵌 Swagger UI 资源");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn localize_swagger_index(bytes: Vec<u8>) -> Result<Vec<u8>, std::string::FromUtf8Error> {
    let html = String::from_utf8(bytes)?
        .replacen("<html lang=\"en\">", "<html lang=\"zh-CN\">", 1)
        .replacen(
            "<head>",
            &format!("<head>\n    {}", swagger_ui_base_element()),
            1,
        )
        .replacen(
            "<title>Swagger UI</title>",
            "<title>RyFrame API 文档</title>",
            1,
        );
    Ok(html.into_bytes())
}

#[cfg(test)]
mod swagger_ui_tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn body_text(response: Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("应能读取 Swagger UI 响应体")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("Swagger UI 文本资源必须是 UTF-8")
    }

    async fn get(path: &str) -> Response {
        swagger_ui_router()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("应能构造 Swagger UI 请求"),
            )
            .await
            .expect("Swagger UI 路由应返回响应")
    }

    #[tokio::test]
    async fn documentation_entry_references_only_same_origin_assets() {
        let response = get("/swagger-ui").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            SWAGGER_UI_NO_CACHE
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("text/html")
        );

        let html = body_text(response).await;
        assert!(html.contains(&swagger_ui_base_element()));
        assert!(html.contains("id=\"swagger-ui\""));
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));

        let mut remaining = html.as_str();
        while let Some(start) = remaining.find("<script") {
            let script = &remaining[start..];
            let end = script.find('>').expect("script 标签必须闭合");
            assert!(
                script[..=end].contains(" src="),
                "Swagger UI 不应包含内联初始化脚本"
            );
            remaining = &script[end + 1..];
        }

        for asset in [
            "swagger-ui.css",
            "index.css",
            "favicon-32x32.png",
            "favicon-16x16.png",
            "swagger-ui-bundle.js",
            "swagger-ui-standalone-preset.js",
            "swagger-initializer.js",
        ] {
            assert!(html.contains(asset), "文档入口未引用内嵌资源：{asset}");
            assert_eq!(
                get(&format!("/swagger-ui/{asset}")).await.status(),
                StatusCode::OK,
                "浏览器无法加载文档资源：{asset}"
            );
        }
    }

    #[tokio::test]
    async fn embedded_static_assets_have_correct_content_types_and_cache_policy() {
        for (asset, expected_content_type) in [
            ("swagger-ui.css", "text/css"),
            ("index.css", "text/css"),
            ("favicon-32x32.png", "image/png"),
            ("favicon-16x16.png", "image/png"),
            ("swagger-ui-bundle.js", "javascript"),
            ("swagger-ui-standalone-preset.js", "javascript"),
        ] {
            let response = get(&format!("/swagger-ui/{asset}")).await;
            assert_eq!(response.status(), StatusCode::OK, "资源不存在：{asset}");
            assert!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains(expected_content_type),
                "资源 Content-Type 错误：{asset}"
            );
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                SWAGGER_UI_STATIC_CACHE
            );
        }

        let initializer = get("/swagger-ui/swagger-initializer.js").await;
        assert_eq!(initializer.status(), StatusCode::OK);
        assert!(
            initializer
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("javascript")
        );
        assert_eq!(
            initializer.headers().get(header::CACHE_CONTROL).unwrap(),
            SWAGGER_UI_NO_CACHE
        );
        let initializer = body_text(initializer).await;
        assert!(initializer.contains("/api/v1/api-docs/openapi.json"));
        assert!(initializer.contains("\"validatorUrl\": \"none\""));
        assert!(!initializer.contains("http://"));
        assert!(!initializer.contains("https://"));
    }

    #[tokio::test]
    async fn documentation_entry_has_no_compatibility_redirect_or_duplicate_index() {
        assert_eq!(get("/swagger-ui/").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            get("/swagger-ui/index.html").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get("/swagger-ui/not-found.js").await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
