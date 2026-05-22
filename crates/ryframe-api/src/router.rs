use axum::{
    middleware,
    routing::{get, post},
    Extension, Router,
};
use axum::extract::State;
use axum::middleware::{from_fn_with_state, Next};
use axum::extract::Request;
use axum::response::Response;
use ryframe_auth::jwt::Claims;
use ryframe_service::system::OnlineUserServiceImpl;
use std::sync::Arc;

use crate::handlers::{auth_handler::{self, AppState}, user_handler, role_handler, permission_handler, menu_handler, dept_handler, post_handler, config_handler, dict_handler, notice_handler, oper_log_handler, login_log_handler, job_handler, generator_handler, common_handler, captcha_handler, profile_handler, online_user_handler};

/// 在线用户跟踪中间件
///
/// 需要在 auth_middleware 之后运行（Claims 已在 extensions 中）。
async fn online_user_tracking(
    State(online_user_service): State<Arc<OnlineUserServiceImpl>>,
    Extension(claims): Extension<Claims>,
    request: Request,
    next: Next,
) -> Response {
    // 更新用户最后访问时间
    online_user_service.touch_user(&claims.jti).await;
    next.run(request).await
}

/// 认证路由
pub fn auth_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/logout", post(auth_handler::logout))
        .route("/me", get(auth_handler::me))
        .route_layer(middleware::from_fn_with_state(
            state.config.clone(),
            ryframe_auth::middleware::auth_middleware,
        ));

    Router::new()
        .route("/login", post(auth_handler::login))
        .route("/refresh", post(auth_handler::refresh))
        .merge(protected)
        .nest("/captcha", captcha_handler::captcha_router(state.clone()))
        .nest("/profile", profile_handler::profile_router(state.clone()))
        .with_state(state)
}

/// API 总路由
pub fn api_router(state: AppState) -> Router {
    let monitor_state = ryframe_monitor::MonitorState {
        db: state.monitor_db.clone(),
    };

    Router::new()
        .nest("/auth", auth_router(state.clone()))
        .nest("/system", system_router(state.clone()))
        .nest("/monitor", ryframe_monitor::monitor_router(monitor_state))
        .nest("/tools", tools_router(state.clone()))
        .nest("/common", common_router(state.clone()))
        // OpenAPI JSON 文档: /api-docs/openapi.json
        // 可使用 https://editor.swagger.io/ 导入查看
        .route("/api-docs/openapi.json", get(crate::openapi::openapi_json))
}

/// 系统管理路由（需认证）
fn system_router(state: AppState) -> Router {
    Router::new()
        .nest("/users", user_handler::user_router(state.clone()))
        .nest("/roles", role_handler::role_router(state.clone()))
        .nest("/permissions", permission_handler::permission_router(state.clone()))
        .nest("/menus", menu_handler::menu_router(state.clone()))
        .nest("/depts", dept_handler::dept_router(state.clone()))
        .nest("/posts", post_handler::post_router(state.clone()))
        .nest("/configs", config_handler::config_router(state.clone()))
        .nest("/dict", dict_handler::dict_router(state.clone()))
        .nest("/notices", notice_handler::notice_router(state.clone()))
        .nest("/operlogs", oper_log_handler::oper_log_router(state.clone()))
        .nest("/loginlogs", login_log_handler::login_log_router(state.clone()))
        .nest("/jobs", job_handler::job_router(state.clone()))
        .nest("/online", online_user_handler::online_user_router(state.clone()))
        .layer(from_fn_with_state(
            state.online_user_service.clone(),
            online_user_tracking,
        ))
        .layer(from_fn_with_state(
            state.config.clone(),
            ryframe_auth::middleware::auth_middleware,
        ))
}

fn tools_router(state: AppState) -> Router {
    Router::new()
        .nest("/gen", generator_handler::generator_router(state.clone()))
}

/// 通用功能路由（文件上传等）
fn common_router(state: AppState) -> Router {
    common_handler::upload_router(state)
}