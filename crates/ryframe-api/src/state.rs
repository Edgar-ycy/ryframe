use std::sync::Arc;

use crate::TrustedProxySet;
use ryframe_application::{
    AuditOutbox, AuthService, JobQueue, JobScheduleService,
    agent::AgentService,
    generated::GeneratedServices,
    ports::tenants::TenantRuntimeReadPort,
    system::{
        content::{ConfigService, DictService, FileService},
        identity::{
            CaptchaStore, DeptService, MenuService, PermissionService, ProfileService, RoleService,
            UserImportService, UserService, WebSocketTicketService,
        },
        operations::{
            DataRetentionService, ExportService, LoginInfoService, MessageService,
            OnlineUserService, OperLogService, OverviewService,
        },
        platform::{
            AuthorizationDiagnosticService, ProductService, ServiceAccountService,
            TenantConfigTransferService, TenantDataMigrationService, TenantService,
            TenantUsageService,
        },
    },
};
use ryframe_kernel::Localizer;

use crate::middleware::idempotency::HttpIdempotencyStore;
use crate::{
    auth_middleware::AuthState, monitor::MonitorState, rate_limit::HttpRateLimiter,
    runtime::RuntimeComponents, settings::HttpRuntimeSettings,
};

#[derive(Clone)]
pub struct IdentityServices {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub role: Arc<RoleService>,
    pub permission: Arc<PermissionService>,
    pub menu: Arc<MenuService>,
    pub dept: Arc<DeptService>,
    pub user_import: Arc<UserImportService>,
    pub profile: Arc<ProfileService>,
    pub captcha: Arc<dyn CaptchaStore>,
    pub websocket_ticket: Arc<WebSocketTicketService>,
}

#[derive(Clone)]
pub struct PlatformServices {
    pub tenant: Arc<TenantService>,
    pub product: Arc<ProductService>,
    pub tenant_data: Arc<dyn TenantRuntimeReadPort>,
    pub tenant_usage: Arc<TenantUsageService>,
    pub service_accounts: Option<Arc<ServiceAccountService>>,
    pub agent: Option<Arc<AgentService>>,
    pub tenant_config_transfer: Arc<TenantConfigTransferService>,
    pub tenant_data_migration: Arc<TenantDataMigrationService>,
    pub authorization_diagnostic: Arc<AuthorizationDiagnosticService>,
}

#[derive(Clone)]
pub struct ContentServices {
    pub generated: GeneratedServices,
    pub config: Arc<ConfigService>,
    pub dict: Arc<DictService>,
    pub file: Arc<FileService>,
}

#[derive(Clone)]
pub struct OperationsServices {
    pub message: Arc<MessageService>,
    pub online_user: Arc<OnlineUserService>,
    pub export: Arc<ExportService>,
    pub oper_log: Arc<OperLogService>,
    pub audit_outbox: Arc<AuditOutbox>,
    pub job_queue: Arc<JobQueue>,
    pub job_schedules: Option<Arc<JobScheduleService>>,
    pub data_retention: Arc<DataRetentionService>,
    pub overview: Arc<OverviewService>,
    pub login_info: Arc<LoginInfoService>,
}

#[derive(Clone)]
pub struct AppServices {
    pub identity: IdentityServices,
    pub platform: PlatformServices,
    pub content: ContentServices,
    pub operations: OperationsServices,
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
