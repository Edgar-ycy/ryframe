use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware,
    middleware::{Next, from_fn_with_state},
    response::{Html, Response},
    routing::{get, post},
};
use ryframe_auth::{jwt::Claims, middleware::AuthState};
use ryframe_middleware::rate_limit::{RateLimitState, user_rate_limit_middleware};
use ryframe_service::system::OnlineUserServiceImpl;
use serde_json::json;

use crate::{
    handlers::{
        auth_handler::{self, AppState},
        captcha_handler, common_handler, config_handler, dept_handler, dict_handler,
        generator_handler, login_log_handler, menu_handler, notice_handler, online_user_handler,
        oper_log_handler, permission_handler, post_handler, profile_handler, role_handler,
        user_handler,
    },
    oper_log_middleware::{OperLogMiddlewareState, oper_log_middleware},
};

/// 在线用户跟踪中间件
///
/// 在 auth_middleware 之后运行（Claims 已在 extensions 中）。
/// 更新用户最后访问时间；若会话被服务重启清除（clear_all_on_startup），自动重新创建。
async fn online_user_tracking(
    State(online_user_service): State<Arc<OnlineUserServiceImpl>>,
    request: Request,
    next: Next,
) -> Response {
    // 尝试从 extensions 获取 Claims，未认证时跳过跟踪
    if let Some(claims) = request.extensions().get::<Claims>() {
        // 提取客户端 IP
        let ip = request
            .headers()
            .get("x-forwarded-for")
            .or_else(|| request.headers().get("x-real-ip"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // 提取 User-Agent
        let user_agent = request
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // 构造会话并确保在线状态（touch + 自动补建）
        let now = chrono::Utc::now();
        online_user_service
            .ensure_user(ryframe_service::system::UserSession {
                token_id: claims.jti.clone(),
                user_id: claims.sub.parse().unwrap_or(0),
                username: claims.username.clone(),
                dept_name: None,
                ipaddr: ip,
                login_location: None,
                browser: ryframe_common::utils::user_agent::parse_browser(user_agent),
                os: ryframe_common::utils::user_agent::parse_os(user_agent),
                login_time: now,
                last_access_time: now,
            })
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
    let oper_log_state = OperLogMiddlewareState::new_arc(state.db.clone());

    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    // 公开路由（无认证，操作日志记录为 "anonymous"）
    let public = Router::new()
        .route("/login", post(auth_handler::login))
        .route("/refresh", post(auth_handler::refresh))
        .layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        ));

    // 受保护路由
    // .layer() 从后往前执行：auth（外层先执行）→ oper_log（内层后执行）→ handler
    let protected = Router::new()
        .route("/logout", post(auth_handler::logout))
        .route("/me", get(auth_handler::me))
        .layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            ryframe_auth::middleware::auth_middleware,
        ));

    // Profile 路由（认证 + 操作日志，中间件在此统一注册）
    // profile_router 不再内嵌 .with_state()
    let profile = Router::new()
        .merge(profile_handler::profile_router())
        .layer(from_fn_with_state(oper_log_state, oper_log_middleware))
        .layer(middleware::from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .nest("/captcha", captcha_handler::captcha_router())
        .nest("/profile", profile)
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
    let monitor_state = ryframe_monitor::MonitorState {
        db: state.monitor_db.clone(),
        redis: state.redis.clone(),
    };

    let auth_state = ryframe_auth::middleware::AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    Router::new()
        .nest("/auth", auth_router(state.clone()))
        .nest(
            "/system",
            system_router(state.clone(), rate_limit_state.clone()),
        )
        .nest(
            "/monitor",
            ryframe_monitor::monitor_router(monitor_state, Some(auth_state)),
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

/// 系统管理路由（需认证 + 用户限流 + 在线跟踪 + 操作日志）
///
/// .layer() 链的语义：后注册的 layer 包裹先注册的，即后注册的先执行（外层先执行）。
/// 执行顺序（从外到内）：
///   1. auth_middleware（最外层，先执行 → 注入 Claims）
///   2. user_rate_limit_middleware（用户级限流，使用 Claims）
///   3. online_user_tracking（在线跟踪，使用 Claims）
///   4. oper_log_middleware（最内层，后执行 → 使用 Claims 记录操作者）
fn system_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    Router::new()
        .nest("/users", user_handler::user_router(state.clone()))
        .nest("/roles", role_handler::role_router(state.clone()))
        .nest(
            "/permissions",
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
            OperLogMiddlewareState::new_arc(state.db.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            state.online_user_service.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ))
}

/// 工具路由（需认证 + 用户限流 + 操作日志）
///
/// 执行顺序（从外到内）：auth → user_rate_limit → oper_log
fn tools_router(state: AppState, rate_limit_state: RateLimitState) -> Router {
    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    Router::new()
        .nest("/gen", generator_handler::generator_router(state.clone()))
        .layer(from_fn_with_state(
            OperLogMiddlewareState::new_arc(state.db.clone()),
            oper_log_middleware,
        ))
        .layer(from_fn_with_state(
            rate_limit_state,
            user_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ))
}

/// 通用功能路由（文件上传等）
/// - 上传路由：公开（无认证，无操作日志以避免大文件请求体缓冲）
/// - 下载路由：需认证 + 操作日志
fn common_router(state: AppState) -> Router {
    use axum::middleware;

    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    let oper_log_state = OperLogMiddlewareState::new_arc(state.db.clone());

    // 上传路由（公开，不记录操作日志以避免大文件 body 缓冲）
    let upload = common_handler::upload_router(state.clone());

    // 下载路由（需认证，记录操作日志）
    let download = common_handler::download_router(state.clone())
        .layer(from_fn_with_state(oper_log_state, oper_log_middleware))
        .layer(middleware::from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ));

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
