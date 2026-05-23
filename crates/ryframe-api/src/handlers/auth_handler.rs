use axum::{extract::State, Extension, Json};
use axum::http::HeaderMap;
use ryframe_auth::jwt::Claims;
use ryframe_common::AppResult;
use ryframe_config::AppConfig;
use ryframe_core::RedisClient;
use ryframe_service::system::{
    ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl,
    JobServiceImpl, LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl,
    OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, RoleServiceImpl,
    UserServiceImpl, ProfileServiceImpl, OnlineUserServiceImpl,
};
use ryframe_service::{AuthServiceImpl, UserInfo};
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use serde_json;
use validator::Validate;
use crate::dto::auth_dto::{LoginRequest, LoginResponse, RefreshRequest};
use crate::handlers::captcha_handler::CaptchaStore;

/// API 共享状态
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<AppConfig>,
    pub context: ryframe_core::AppContext,
    pub auth_service: Arc<AuthServiceImpl>,
    pub user_service: Arc<UserServiceImpl>,
    pub role_service: Arc<RoleServiceImpl>,
    pub permission_service: Arc<PermissionServiceImpl>,
    pub menu_service: Arc<MenuServiceImpl>,
    pub dept_service: Arc<DeptServiceImpl>,
    pub post_service: Arc<PostServiceImpl>,
    pub config_service: Arc<ConfigServiceImpl>,
    pub dict_service: Arc<DictServiceImpl>,
    pub notice_service: Arc<NoticeServiceImpl>,
    pub oper_log_service: Arc<OperLogServiceImpl>,
    pub login_info_service: Arc<LoginInfoServiceImpl>,
    pub job_service: Arc<JobServiceImpl>,
    pub generator_service: Arc<GeneratorServiceImpl>,
    pub profile_service: Arc<ProfileServiceImpl>,
    pub online_user_service: Arc<OnlineUserServiceImpl>,
    pub captcha_store: CaptchaStore,
    pub scheduler: Arc<ryframe_task::TaskScheduler>,
    pub monitor_db: DatabaseConnection,
    pub redis: Option<RedisClient>,
    /// 从库连接池列表（用于读写分离的读操作）
    pub replica_dbs: Vec<DatabaseConnection>,
}

/// 用户登录
///
/// POST /api/v1/auth/login
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "认证",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功", body = LoginResponse),
        (status = 400, description = "参数校验失败"),
        (status = 401, description = "用户名或密码错误")
    )
)]
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<LoginResponse>> {
    req.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;

    let ip = ryframe_common::utils::ip::get_client_ip(&headers, "unknown");
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match state.auth_service.login(&state.db, &req.username, &req.password).await {
        Ok(result) => {
            let _ = state.login_info_service.record_login(
                &state.db, &req.username, &ip,
                Some(user_agent), None,
                ryframe_db::entities::login_info::Model::STATUS_SUCCESS, None,
            ).await;

            // 添加在线用户
            let now = chrono::Utc::now();
            state.online_user_service.add_user(
                ryframe_service::system::UserSession {
                    token_id: result.token_id.clone(),
                    user_id: result.user_info.id,
                    username: result.user_info.username.clone(),
                    dept_name: None, // 可后续查询部门
                    ipaddr: ip.to_string(),
                    login_location: None,
                    browser: parse_browser(user_agent),
                    os: parse_os(user_agent),
                    login_time: now,
                    last_access_time: now,
                }
            ).await;

            Ok(Json(LoginResponse::from(result)))
        }
        Err(e) => {
            let _ = state.login_info_service.record_login(
                &state.db, &req.username, &ip,
                Some(user_agent), None,
                ryframe_db::entities::login_info::Model::STATUS_FAIL,
                Some(&e.to_string()),
            ).await;
            Err(e)
        }
    }
}

/// 用户登出
///
/// POST /api/v1/auth/logout
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "认证",
    responses(
        (status = 200, description = "登出成功")
    ),
    security(("bearer" = []))
)]
pub async fn logout(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> AppResult<Json<serde_json::Value>> {
    // 从在线用户中移除
    state.online_user_service.remove_user(&claims.jti).await;
    Ok(Json(serde_json::json!({"message": "登出成功"})))
}

/// 刷新令牌
///
/// POST /api/v1/auth/refresh
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "认证",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "刷新成功", body = LoginResponse),
        (status = 401, description = "令牌无效或已过期")
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> AppResult<Json<LoginResponse>> {
    let ip = ryframe_common::utils::ip::get_client_ip(&headers, "unknown");
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match state.auth_service.refresh_token(&state.db, &req.refresh_token).await {
        Ok(result) => {
            let _ = state.login_info_service.record_login(
                &state.db, &result.user_info.username, &ip,
                Some(user_agent), None,
                ryframe_db::entities::login_info::Model::STATUS_SUCCESS,
                Some("令牌刷新"),
            ).await;
            Ok(Json(LoginResponse::from(result)))
        }
        Err(e) => {
            let _ = state.login_info_service.record_login(
                &state.db, "unknown", &ip,
                Some(user_agent), None,
                ryframe_db::entities::login_info::Model::STATUS_FAIL,
                Some(&e.to_string()),
            ).await;
            Err(e)
        }
    }
}

/// 获取当前用户信息
///
/// GET /api/v1/auth/me
/// 需要认证中间件预先注入 Claims 到 request extensions。
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "认证",
    responses(
        (status = 200, description = "用户信息", body = UserInfo),
        (status = 401, description = "未认证")
    ),
    security(("bearer" = []))
)]
pub async fn me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> AppResult<Json<ryframe_service::UserInfo>> {
    let user_id = claims.sub.parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Authentication("令牌无效".into()))?;

    let user_info = state
        .auth_service
        .get_current_user(&state.db, user_id)
        .await?;

    Ok(Json(user_info))
}

/// 从 User-Agent 解析浏览器名称
fn parse_browser(ua: &str) -> Option<String> {
    if ua.is_empty() {
        return None;
    }
    let ua_lower = ua.to_lowercase();
    let browser = if ua_lower.contains("edg/") {
        "Edge"
    } else if ua_lower.contains("chrome/") && !ua_lower.contains("edg/") {
        "Chrome"
    } else if ua_lower.contains("firefox/") {
        "Firefox"
    } else if ua_lower.contains("safari/") && !ua_lower.contains("chrome/") {
        "Safari"
    } else if ua_lower.contains("opera") || ua_lower.contains("opr/") {
        "Opera"
    } else {
        "Other"
    };
    Some(browser.to_string())
}

/// 从 User-Agent 解析操作系统名称
fn parse_os(ua: &str) -> Option<String> {
    if ua.is_empty() {
        return None;
    }
    let os = if ua.contains("Windows NT 10") {
        "Windows 10"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Mac OS X") {
        "macOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else {
        "Other"
    };
    Some(os.to_string())
}