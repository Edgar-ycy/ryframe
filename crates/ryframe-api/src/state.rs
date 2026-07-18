use std::sync::Arc;

use ryframe_auth::middleware::AuthState;
use ryframe_common::utils::ip::TrustedProxySet;
use ryframe_config::AppConfig;
use ryframe_core::{RedisClient, TokenBlacklist};
use ryframe_middleware::RateLimiter;
use ryframe_monitor::MonitorState;
use ryframe_service::{
    AuthService,
    system::{
        CaptchaStore, ConfigService, DeptService, DictService, FileService, GeneratorService,
        LoginInfoService, MenuService, NoticeService, OnlineUserService, OperLogService,
        PermissionService, PostService, ProfileService, RoleService, TenantService, UserService,
    },
};

use crate::runtime::RuntimeComponents;

#[derive(Clone)]
pub struct AppServices {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub role: Arc<RoleService>,
    pub tenant: Arc<TenantService>,
    pub permission: Arc<PermissionService>,
    pub menu: Arc<MenuService>,
    pub dept: Arc<DeptService>,
    pub post: Arc<PostService>,
    pub config: Arc<ConfigService>,
    pub dict: Arc<DictService>,
    pub notice: Arc<NoticeService>,
    pub oper_log: Arc<OperLogService>,
    pub login_info: Arc<LoginInfoService>,
    pub generator: Arc<GeneratorService>,
    pub profile: Arc<ProfileService>,
    pub file: Arc<FileService>,
    pub online_user: Arc<OnlineUserService>,
    pub captcha: CaptchaStore,
}

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthState,
    pub monitor: MonitorState,
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
    pub redis: Option<RedisClient>,
    pub token_blacklist: TokenBlacklist,
    pub rate_limiter: Arc<RateLimiter>,
    pub trusted_proxies: TrustedProxySet,
    pub runtime: RuntimeComponents,
}
