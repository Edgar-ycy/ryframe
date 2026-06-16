use std::{net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use ryframe_auth::jwt::Claims;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_config::AppConfig;
use ryframe_core::{RedisClient, TokenBlacklist};
use ryframe_middleware::RateLimiter;
use ryframe_service::{
    AuthServiceImpl, UserInfo,
    system::{
        CaptchaStore, ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl,
        LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl, OnlineUserServiceImpl,
        OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, ProfileServiceImpl,
        RoleServiceImpl, UserServiceImpl,
    },
};
use sea_orm::DatabaseConnection;
use validator::Validate;

use crate::dto::auth_dto::{LoginRequest, LoginResponse, RefreshRequest};
use crate::runtime::RuntimeComponents;

/// API 共享状态
#[derive(Clone)]
pub struct AppState {
    /// 主库连接（向后兼容，指向 primary 数据源）
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
    pub generator_service: Arc<GeneratorServiceImpl>,
    pub profile_service: Arc<ProfileServiceImpl>,
    pub online_user_service: Arc<OnlineUserServiceImpl>,
    pub captcha_store: CaptchaStore,
    pub monitor_db: DatabaseConnection,
    pub redis: Option<RedisClient>,
    /// Token 黑名单（JWT 主动撤销）
    pub token_blacklist: TokenBlacklist,
    /// 从库连接池列表（用于读写分离的读操作，向后兼容）
    pub replica_dbs: Vec<DatabaseConnection>,
    /// 限流器（用于验证码等接口的细粒度限流）
    pub rate_limiter: Arc<RateLimiter>,
    /// 对象存储（本地/MinIO/S3，通过配置切换）
    pub object_storage: Arc<dyn ryframe_common::utils::ObjectStorage>,
    /// Runtime components shared by business workflows.
    pub runtime: RuntimeComponents,
}

impl AppState {
    /// 获取写库连接（向后兼容：始终返回 primary）
    pub fn write_db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// 获取读库连接（向后兼容：轮询 replicas，无 replicas 回退 primary）
    pub fn read_db(&self) -> &DatabaseConnection {
        if self.replica_dbs.is_empty() {
            &self.db
        } else {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let idx = COUNTER.fetch_add(1, Ordering::Relaxed) % self.replica_dbs.len();
            &self.replica_dbs[idx]
        }
    }
}

// ==================== 登录辅助：参数提取 ====================

/// 提取客户端 IP
fn extract_ip(headers: &HeaderMap, remote_addr: &str) -> String {
    ryframe_common::utils::ip::get_client_ip(headers, remote_addr)
}

/// 提取 User-Agent
fn extract_user_agent(headers: &HeaderMap) -> &str {
    headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// 验证码校验（HTTP 安全校验，保留在 Handler）
async fn verify_captcha_if_enabled(state: &AppState, req: &LoginRequest) -> AppResult<()> {
    let captcha_enabled = state
        .config_service
        .find_by_key(&state.db, "sys.account.captchaEnabled")
        .await
        .ok()
        .flatten()
        .map(|c| c.value == "true")
        .unwrap_or(true);
    if !captcha_enabled {
        return Ok(());
    }

    let captcha_id = req
        .captcha_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("验证码ID不能为空".into()))?;
    let captcha_code = req
        .captcha_code
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("验证码不能为空".into()))?;
    let valid = state.captcha_store.verify(captcha_id, captcha_code).await;
    if !valid {
        return Err(AppError::Validation("验证码错误或已过期".into()));
    }
    Ok(())
}

// ==================== 认证接口 ====================

/// 记录登录成功日志
async fn record_login_success(state: &AppState, username: &str, ip: &str, ua: &str) {
    if let Err(e) = state
        .login_info_service
        .record_login(
            &state.db,
            username,
            ip,
            ryframe_common::utils::user_agent::parse_browser(ua).as_deref(),
            ryframe_common::utils::user_agent::parse_os(ua).as_deref(),
            ryframe_db::entities::login_info::Model::STATUS_SUCCESS,
            None,
        )
        .await
    {
        tracing::error!("记录登录成功日志失败: {}", e);
    }
}

/// 记录登录失败日志
async fn record_login_failure_log(
    state: &AppState,
    username: &str,
    ip: &str,
    ua: &str,
    err: &ryframe_common::AppError,
) {
    if let Err(e) = state
        .login_info_service
        .record_login(
            &state.db,
            username,
            ip,
            ryframe_common::utils::user_agent::parse_browser(ua).as_deref(),
            ryframe_common::utils::user_agent::parse_os(ua).as_deref(),
            ryframe_db::entities::login_info::Model::STATUS_FAIL,
            Some(&err.to_string()),
        )
        .await
    {
        tracing::error!("记录登录失败日志失败: {}", e);
    }
}

/// 添加在线用户
async fn add_online_user(
    state: &AppState,
    result: &ryframe_service::LoginResult,
    ip: &str,
    ua: &str,
) {
    use ryframe_service::system::UserSession;

    let user_id: i64 = result.user_info.id.parse().unwrap_or(0);
    let dept_name = state
        .user_service
        .find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.dept_name);
    let login_location = ryframe_common::utils::ip::get_ip_location(ip);
    let now = chrono::Utc::now();

    state
        .online_user_service
        .add_user(UserSession {
            token_id: result.token_id.clone(),
            user_id,
            username: result.user_info.username.clone(),
            dept_name,
            ipaddr: ip.to_string(),
            login_location,
            browser: ryframe_common::utils::user_agent::parse_browser(ua),
            os: ryframe_common::utils::user_agent::parse_os(ua),
            login_time: now,
            last_access_time: now,
        })
        .await;
}

/// 检查用户是否被强制下线
async fn check_force_logout(state: &AppState, refresh_token: &str) -> AppResult<()> {
    if let Ok(claims) =
        ryframe_auth::jwt::decode_token(refresh_token, &state.config.auth.jwt_secret)
    {
        let force_logout_key = format!("force_logout:user:{}", claims.sub);
        if state
            .token_blacklist
            .is_blacklisted(&force_logout_key)
            .await
        {
            return Err(AppError::Authentication(
                "账号已被强制下线，请重新登录".into(),
            ));
        }
    }
    Ok(())
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<ApiResponse<LoginResponse>>> {
    req.validate()?;

    // HTTP 安全校验：验证码
    verify_captcha_if_enabled(&state, &req).await?;

    // HTTP 参数提取
    let ip = extract_ip(&headers, &addr.to_string());
    let ua = extract_user_agent(&headers);

    // 暴力破解防护（委托 Service）
    state
        .auth_service
        .check_brute_force(&req.username, &ip)
        .await?;

    // 认证（委托 Service）
    match state
        .auth_service
        .login(&state.db, &req.username, &req.password)
        .await
    {
        Ok(result) => {
            // 登录成功：清除失败计数
            state
                .auth_service
                .clear_login_failures(&req.username, &ip)
                .await;
            // 清除强退黑名单
            let force_logout_key = format!("force_logout:user:{}", result.user_info.id);
            state.token_blacklist.remove(&force_logout_key).await;
            // 记录登录日志
            record_login_success(&state, &req.username, &ip, ua).await;
            // 添加在线用户
            add_online_user(&state, &result, &ip, ua).await;

            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            // 登录失败：记录失败次数 + 记录失败日志
            state
                .auth_service
                .record_login_failure(&req.username, &ip)
                .await;
            record_login_failure_log(&state, &req.username, &ip, ua, &e).await;
            Err(e)
        }
    }
}

/// 用户登出
///
/// POST /api/v1/auth/logout
/// 将当前 token 加入黑名单，实现 JWT 主动撤销。
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
) -> AppResult<Json<ApiResponse<()>>> {
    // 将当前 token 加入黑名单（JWT 主动撤销）
    let now = chrono::Utc::now().timestamp() as usize;
    let remaining = if claims.exp > now {
        (claims.exp - now) as u64
    } else {
        0
    };
    if remaining > 0 {
        state
            .token_blacklist
            .blacklist(&claims.jti, remaining)
            .await;
    }

    // 从在线用户中移除
    state.online_user_service.remove_user(&claims.jti).await;
    Ok(Json(ApiResponse::<()>::success_no_data()))
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RefreshRequest>,
) -> AppResult<Json<ApiResponse<LoginResponse>>> {
    // 检查强退
    check_force_logout(&state, &req.refresh_token).await?;

    // HTTP 参数提取
    let ip = extract_ip(&headers, &addr.to_string());
    let ua = extract_user_agent(&headers);

    // 刷新令牌（委托 Service）
    match state
        .auth_service
        .refresh_token(&state.db, &req.refresh_token)
        .await
    {
        Ok(result) => {
            record_login_success(&state, &result.user_info.username, &ip, ua).await;
            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            record_login_failure_log(&state, "unknown", &ip, ua, &e).await;
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
) -> AppResult<Json<ApiResponse<ryframe_service::UserInfo>>> {
    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Authentication("令牌无效".into()))?;

    let user_info = state
        .auth_service
        .get_current_user(&state.db, user_id)
        .await?;

    Ok(Json(ApiResponse::success(user_info)))
}
