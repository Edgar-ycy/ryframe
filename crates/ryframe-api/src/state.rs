use std::sync::Arc;

use crate::TrustedProxySet;
use ryframe_application::{
    AuditOutbox, AuthService, JobQueue, JobScheduleService,
    agent::AgentService,
    ports::tenants::TenantRuntimeReadPort,
    system::{
        AuthorizationDiagnosticService, CaptchaStore, ConfigService, DataRetentionService,
        DeptService, DictService, ExportService, FileService, LoginInfoService, MenuService,
        MessageService, NoticeService, OnlineUserService, OperLogService, OverviewService,
        PermissionService, PostService, ProductService, ProfileService, RoleService,
        ServiceAccountService, TenantConfigTransferService, TenantDataMigrationService,
        TenantService, TenantUsageService, UserImportService, UserService, WebSocketTicketService,
    },
};
use ryframe_kernel::Localizer;

use crate::middleware::idempotency::HttpIdempotencyStore;
use crate::{
    auth_middleware::AuthState, monitor::MonitorState, rate_limit::HttpRateLimiter,
    runtime::RuntimeComponents, settings::HttpRuntimeSettings,
};

#[derive(Clone)]
pub struct AppServices {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub role: Arc<RoleService>,
    pub tenant: Arc<TenantService>,
    pub product: Arc<ProductService>,
    pub tenant_data: Arc<dyn TenantRuntimeReadPort>,
    pub tenant_usage: Arc<TenantUsageService>,
    pub service_accounts: Option<Arc<ServiceAccountService>>,
    pub agent: Option<Arc<AgentService>>,
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
    pub job_schedules: Option<Arc<JobScheduleService>>,
    pub data_retention: Arc<DataRetentionService>,
    pub user_import: Arc<UserImportService>,
    pub tenant_config_transfer: Arc<TenantConfigTransferService>,
    pub tenant_data_migration: Arc<TenantDataMigrationService>,
    pub authorization_diagnostic: Arc<AuthorizationDiagnosticService>,
    pub overview: Arc<OverviewService>,
    pub login_info: Arc<LoginInfoService>,
    pub profile: Arc<ProfileService>,
    pub file: Arc<FileService>,
    pub online_user: Arc<OnlineUserService>,
    pub captcha: Arc<dyn CaptchaStore>,
}

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthState,
    pub monitor: MonitorState,
    pub settings: Arc<HttpRuntimeSettings>,
    pub localizer: Arc<Localizer>,
    pub services: Arc<AppServices>,
    pub redis_connected: bool,
    pub idempotency_store: Option<Arc<dyn HttpIdempotencyStore>>,
    pub message_hub: Arc<crate::message_socket::MessageHub>,
    pub rate_limiter: Arc<dyn HttpRateLimiter>,
    pub trusted_proxies: TrustedProxySet,
    pub runtime: RuntimeComponents,
}

impl AppState {
    /// 判断 WebSocket 票据是否处于启动时已经确认的受控降级状态。
    ///
    /// Redis optional 模式在启动时连接失败会没有客户端；这与显式禁用 Redis 一样
    /// 是可预测的实时通道不可用状态。运行期 Redis I/O 失败仍不在此列，保留原有
    /// 可观测性。
    pub(crate) fn websocket_ticket_is_expected_unavailable(&self) -> bool {
        !self.settings.messaging.enabled || !self.redis_connected
    }
}
