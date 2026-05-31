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
use ryframe_service::system::OnlineUserServiceImpl;
use serde_json::json;

use crate::{
    handlers::{
        auth_handler::{self, AppState},
        captcha_handler, common_handler, config_handler, dept_handler, dict_handler,
        generator_handler, job_handler, login_log_handler, menu_handler, notice_handler,
        online_user_handler, oper_log_handler, permission_handler, post_handler, profile_handler,
        role_handler, user_handler,
    },
    oper_log_middleware::{OperLogMiddlewareState, oper_log_middleware},
};

/// 在线用户跟踪中间件
///
/// 在 auth_middleware 之后运行（Claims 已在 extensions 中）。
/// 优雅处理未认证请求（跳过跟踪）。
async fn online_user_tracking(
    State(online_user_service): State<Arc<OnlineUserServiceImpl>>,
    request: Request,
    next: Next,
) -> Response {
    // 尝试从 extensions 获取 Claims，未认证时跳过跟踪
    if let Some(claims) = request.extensions().get::<Claims>() {
        online_user_service.touch_user(&claims.jti).await;
    }
    next.run(request).await
}

/// 认证路由
pub fn auth_router(state: AppState) -> Router {
    let oper_log_state = Arc::new(OperLogMiddlewareState {
        db: state.db.clone(),
    });

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

    // 受保护路由：auth → oper_log → handler
    // layer 从下到上执行，最后注册的最外层先执行
    let protected = Router::new()
        .route("/logout", post(auth_handler::logout))
        .route("/me", get(auth_handler::me))
        .route_layer(from_fn_with_state(
            oper_log_state.clone(),
            oper_log_middleware,
        ))
        .route_layer(middleware::from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ));

    // Profile 路由（auth + oper_log 在 profile_router 内部处理）
    let profile = profile_handler::profile_router(state.clone());

    Router::new()
        .merge(public)
        .merge(protected)
        .nest("/captcha", captcha_handler::captcha_router(state.clone()))
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
pub fn api_router(state: AppState) -> Router {
    let monitor_state = ryframe_monitor::MonitorState {
        db: state.monitor_db.clone(),
        redis: state.redis.clone(),
    };

    Router::new()
        .nest("/auth", auth_router(state.clone()))
        .nest("/system", system_router(state.clone()))
        .nest("/monitor", ryframe_monitor::monitor_router(monitor_state))
        .nest("/tools", tools_router(state.clone()))
        .nest("/common", common_router(state.clone()))
        // API 版本信息端点
        .route("/version", get(api_version))
        // OpenAPI JSON 文档: /api-docs/openapi.json
        .route("/api-docs/openapi.json", get(crate::openapi::openapi_json))
        // Swagger UI 交互文档: /swagger-ui
        .route("/swagger-ui", get(swagger_ui))
}

/// 系统管理路由（需认证）
/// layer 从下到上执行（最后注册的最外层先执行）：
///   1. auth_middleware（最外层，先执行 → 注入 Claims）
///   2. online_user_tracking（使用 Claims）
///   3. oper_log_middleware（最内层，最后执行 → 使用 Claims 记录操作者）
fn system_router(state: AppState) -> Router {
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
        .nest("/jobs", job_handler::job_router(state.clone()))
        .nest(
            "/online",
            online_user_handler::online_user_router(state.clone()),
        )
        .layer(from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ))
        .layer(from_fn_with_state(
            state.online_user_service.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            Arc::new(OperLogMiddlewareState {
                db: state.db.clone(),
            }),
            oper_log_middleware,
        ))
}

/// 工具路由（需认证）
/// layer 从下到上执行：auth → oper_log
fn tools_router(state: AppState) -> Router {
    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    Router::new()
        .nest("/gen", generator_handler::generator_router(state.clone()))
        .layer(from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ))
        .layer(from_fn_with_state(
            Arc::new(OperLogMiddlewareState {
                db: state.db.clone(),
            }),
            oper_log_middleware,
        ))
}

/// 通用功能路由（文件上传等）
/// - 上传路由：公开（需要时可在 handler 内做认证）
/// - 下载路由：需认证
fn common_router(state: AppState) -> Router {
    use axum::middleware;

    let auth_state = AuthState {
        config: state.config.clone(),
        blacklist: state.token_blacklist.clone(),
    };

    let oper_log_state = Arc::new(OperLogMiddlewareState {
        db: state.db.clone(),
    });

    // 上传路由（公开，记录操作日志）
    let upload = common_handler::upload_router(state.clone()).layer(from_fn_with_state(
        oper_log_state.clone(),
        oper_log_middleware,
    ));

    // 下载路由（需认证，记录操作日志）
    let download = common_handler::download_router(state.clone())
        .layer(from_fn_with_state(oper_log_state, oper_log_middleware))
        .route_layer(middleware::from_fn_with_state(
            auth_state,
            ryframe_auth::middleware::auth_middleware,
        ));

    Router::new().merge(upload).merge(download)
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
