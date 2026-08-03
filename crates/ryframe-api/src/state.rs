use std::sync::Arc;

use ryframe_auth::middleware::AuthState;
use ryframe_config::AppConfig;
use ryframe_core::{RedisClient, TokenBlacklist};
use ryframe_i18n::Localizer;
use ryframe_middleware::RateLimiter;
use ryframe_monitor::MonitorState;
use ryframe_service::{
    AuditOutbox, AuthService, JobQueue,
    system::{
        CaptchaStore, ConfigService, DeptService, DictService, ExportService, FileService,
        GeneratorService, LoginInfoService, MenuService, MessageService, NoticeService,
        OnlineUserService, OperLogService, PermissionService, PostService, ProfileService,
        RoleService, TenantService, UserService, WebSocketTicketService,
    },
};
use ryframe_utils::ip::TrustedProxySet;

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
    pub export: Arc<ExportService>,
    pub notice: Arc<NoticeService>,
    pub message: Arc<MessageService>,
    pub websocket_ticket: Arc<WebSocketTicketService>,
    pub oper_log: Arc<OperLogService>,
    pub audit_outbox: Arc<AuditOutbox>,
    pub job_queue: Arc<JobQueue>,
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
    pub localizer: Arc<Localizer>,
    pub services: Arc<AppServices>,
    pub redis: Option<RedisClient>,
    pub message_hub: Arc<crate::message_socket::MessageHub>,
    pub token_blacklist: TokenBlacklist,
    pub rate_limiter: Arc<RateLimiter>,
    pub trusted_proxies: TrustedProxySet,
    pub runtime: RuntimeComponents,
}
