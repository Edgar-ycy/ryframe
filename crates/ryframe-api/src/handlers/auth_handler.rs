use std::{net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use ryframe_auth::jwt::Claims;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_config::AppConfig;
use ryframe_core::{DataSourceManager, RedisClient, TokenBlacklist};
use ryframe_middleware::RateLimiter;
use ryframe_service::{
    AuthServiceImpl, UserInfo,
    system::{
        ConfigServiceImpl, DeptServiceImpl, DictServiceImpl, GeneratorServiceImpl, JobServiceImpl,
        LoginInfoServiceImpl, MenuServiceImpl, NoticeServiceImpl, OnlineUserServiceImpl,
        OperLogServiceImpl, PermissionServiceImpl, PostServiceImpl, ProfileServiceImpl,
        RoleServiceImpl, UserServiceImpl,
    },
};
use sea_orm::DatabaseConnection;
use validator::Validate;

use ryframe_common::utils::user_agent::{parse_browser, parse_os};

use crate::{
    dto::auth_dto::{LoginRequest, LoginResponse, RefreshRequest},
    handlers::captcha_handler::CaptchaStore,
};

/// API 共享状态
#[derive(Clone)]
pub struct AppState {
    /// 多数据源管理器（新增）
    ///
    /// 配合 `#[datasource("name")]` 注解实现透明数据源切换。
    pub datasource_manager: DataSourceManager,

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
    pub job_service: Arc<JobServiceImpl>,
    pub generator_service: Arc<GeneratorServiceImpl>,
    pub profile_service: Arc<ProfileServiceImpl>,
    pub online_user_service: Arc<OnlineUserServiceImpl>,
    pub captcha_store: CaptchaStore,
    pub scheduler: Arc<ryframe_task::TaskScheduler>,
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

/// 检查登录暴力破解
///
/// 使用 Redis 记录失败次数，按用户名/IP 维度分别限流。
/// - 连续失败超限后锁定指定分钟
/// - Redis 不可用时降级为无条件放行
async fn check_brute_force(
    redis: &Option<RedisClient>,
    config: &Arc<AppConfig>,
    username: &str,
    ip: &str,
) -> AppResult<()> {
    let max_attempts = config.auth.max_login_attempts;
    let lockout_seconds = (config.auth.lockout_duration_minutes * 60) as u64;

    if let Some(redis) = redis {
        // 按用户名限流
        let user_key = format!("login_fail:user:{}", username);
        if let Ok(Some(count)) = redis.get(&user_key).await
            && let Ok(c) = count.parse::<u32>()
            && c >= max_attempts
        {
            let ttl = redis.ttl(&user_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "账户已被锁定，请 {} 秒后再试",
                    ttl
                )));
            }
        }

        // 按 IP 限流
        let ip_key = format!("login_fail:ip:{}", ip);
        if let Ok(Some(count)) = redis.get(&ip_key).await
            && let Ok(c) = count.parse::<u32>()
            && c >= max_attempts * 2
        {
            let ttl = redis.ttl(&ip_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "IP 已被临时限制，请 {} 秒后再试",
                    ttl
                )));
            }
        }
    }

    Ok(())
}

/// 记录登录失败（递增 Redis 计数器）
async fn record_login_failure(
    redis: &Option<RedisClient>,
    config: &Arc<AppConfig>,
    username: &str,
    ip: &str,
) {
    if let Some(redis) = redis {
        let lockout_seconds = (config.auth.lockout_duration_minutes * 60) as u64;
        let user_key = format!("login_fail:user:{}", username);
        let ip_key = format!("login_fail:ip:{}", ip);

        // 递增计数器并设置过期时间
        let _ = redis.incr(&user_key).await;
        let _ = redis.expire(&user_key, lockout_seconds).await;
        let _ = redis.incr(&ip_key).await;
        let _ = redis.expire(&ip_key, lockout_seconds).await;
    }
}

/// 登录成功后清除失败计数
async fn clear_login_failures(redis: &Option<RedisClient>, username: &str, ip: &str) {
    if let Some(redis) = redis {
        let user_key = format!("login_fail:user:{}", username);
        let ip_key = format!("login_fail:ip:{}", ip);
        let _ = redis.del(&user_key).await;
        let _ = redis.del(&ip_key).await;
    }
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

    // 检查验证码开关：开启时校验验证码
    {
        let captcha_enabled = state
            .config_service
            .find_by_key(&state.db, "sys.account.captchaEnabled")
            .await
            .ok()
            .flatten()
            .map(|c| c.value == "true")
            .unwrap_or(true);
        if captcha_enabled {
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
        }
    }

    let remote_addr = addr.to_string();
    let ip = ryframe_common::utils::ip::get_client_ip(&headers, &remote_addr);
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // 暴力破解防护：登录前检查
    check_brute_force(&state.redis, &state.config, &req.username, &ip).await?;

    match state
        .auth_service
        .login(&state.db, &req.username, &req.password)
        .await
    {
        Ok(result) => {
            // 登录成功：清除失败计数
            clear_login_failures(&state.redis, &req.username, &ip).await;

            if let Err(e) = state
                .login_info_service
                .record_login(
                    &state.db,
                    &req.username,
                    &ip,
                    parse_browser(user_agent).as_deref(),
                    parse_os(user_agent).as_deref(),
                    ryframe_db::entities::login_info::Model::STATUS_SUCCESS,
                    None,
                )
                .await
            {
                tracing::error!("记录登录成功日志失败: {}", e);
            }

            // 查询用户部门名称
            let dept_name = state
                .user_service
                .find_by_id(&state.db, result.user_info.id)
                .await
                .ok()
                .flatten()
                .and_then(|u| u.dept_name);

            // IP 归属地解析
            let login_location = ryframe_common::utils::ip::get_ip_location(&ip);

            // 添加在线用户
            let now = chrono::Utc::now();
            state
                .online_user_service
                .add_user(ryframe_service::system::UserSession {
                    token_id: result.token_id.clone(),
                    user_id: result.user_info.id,
                    username: result.user_info.username.clone(),
                    dept_name,
                    ipaddr: ip.to_string(),
                    login_location,
                    browser: ryframe_common::utils::user_agent::parse_browser(user_agent),
                    os: ryframe_common::utils::user_agent::parse_os(user_agent),
                    login_time: now,
                    last_access_time: now,
                })
                .await;

            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            // 登录失败：记录失败次数
            record_login_failure(&state.redis, &state.config, &req.username, &ip).await;

            if let Err(err) = state
                .login_info_service
                .record_login(
                    &state.db,
                    &req.username,
                    &ip,
                    parse_browser(user_agent).as_deref(),
                    parse_os(user_agent).as_deref(),
                    ryframe_db::entities::login_info::Model::STATUS_FAIL,
                    Some(&e.to_string()),
                )
                .await
            {
                tracing::error!("记录登录失败日志失败: {}", err);
            }
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
    let remote_addr = addr.to_string();
    let ip = ryframe_common::utils::ip::get_client_ip(&headers, &remote_addr);
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match state
        .auth_service
        .refresh_token(&state.db, &req.refresh_token)
        .await
    {
        Ok(result) => {
            if let Err(e) = state
                .login_info_service
                .record_login(
                    &state.db,
                    &result.user_info.username,
                    &ip,
                    parse_browser(user_agent).as_deref(),
                    parse_os(user_agent).as_deref(),
                    ryframe_db::entities::login_info::Model::STATUS_SUCCESS,
                    Some("令牌刷新"),
                )
                .await
            {
                tracing::error!("记录令牌刷新日志失败: {}", e);
            }
            Ok(Json(ApiResponse::success(LoginResponse::from(result))))
        }
        Err(e) => {
            if let Err(err) = state
                .login_info_service
                .record_login(
                    &state.db,
                    "unknown",
                    &ip,
                    parse_browser(user_agent).as_deref(),
                    parse_os(user_agent).as_deref(),
                    ryframe_db::entities::login_info::Model::STATUS_FAIL,
                    Some(&e.to_string()),
                )
                .await
            {
                tracing::error!("记录令牌刷新失败日志失败: {}", err);
            }
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
