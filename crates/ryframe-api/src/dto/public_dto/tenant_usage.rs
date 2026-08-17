use chrono::{DateTime, Utc};
use ryframe_service::system::{
    QuotaUsage as ServiceQuotaUsage, RequestWindowUsage as ServiceRequestWindowUsage,
    TenantAuxiliaryUsage as ServiceTenantAuxiliaryUsage,
    TenantCapacityVo as ServiceTenantCapacityVo, TenantUsageVo as ServiceTenantUsageVo,
};
use serde::Serialize;
use utoipa::ToSchema;

/// 单项租户配额用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantQuotaUsageVo {
    pub used: u64,
    /// `None` 表示该资源不受配额限制。
    pub limit: Option<u64>,
    /// 使用率基点，10000 表示 100%；无限制时为 `None`。
    pub percentage_basis_points: Option<u32>,
    pub status: String,
}

impl From<ServiceQuotaUsage> for TenantQuotaUsageVo {
    fn from(value: ServiceQuotaUsage) -> Self {
        Self {
            used: value.used,
            limit: value.limit,
            percentage_basis_points: value.percentage_basis_points,
            status: value.status,
        }
    }
}

/// 租户当前请求限流窗口用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantRequestWindowUsageVo {
    /// Redis 不可用时为 `None`。
    pub current: Option<u64>,
    /// `None` 表示租户请求限流未启用。
    pub limit: Option<u64>,
    pub percentage_basis_points: Option<u32>,
    /// Redis 不可用时为 `None`。
    pub remaining_secs: Option<u64>,
    pub status: String,
}

impl From<ServiceRequestWindowUsage> for TenantRequestWindowUsageVo {
    fn from(value: ServiceRequestWindowUsage) -> Self {
        Self {
            current: value.current,
            limit: value.limit,
            percentage_basis_points: value.percentage_basis_points,
            remaining_secs: value.remaining_secs,
            status: value.status,
        }
    }
}

/// 租户后台运行状态汇总。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantAuxiliaryUsageVo {
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub dead_jobs: u64,
    pub enabled_schedules: u64,
    pub active_user_imports: u64,
    pub cron_enabled: bool,
}

impl From<ServiceTenantAuxiliaryUsage> for TenantAuxiliaryUsageVo {
    fn from(value: ServiceTenantAuxiliaryUsage) -> Self {
        Self {
            pending_jobs: value.pending_jobs,
            running_jobs: value.running_jobs,
            dead_jobs: value.dead_jobs,
            enabled_schedules: value.enabled_schedules,
            active_user_imports: value.active_user_imports,
            cron_enabled: value.cron_enabled,
        }
    }
}

/// 租户容量与当前窗口用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantUsageVo {
    pub tenant_id: String,
    pub calculated_at: DateTime<Utc>,
    pub users: TenantQuotaUsageVo,
    pub roles: TenantQuotaUsageVo,
    pub storage: TenantQuotaUsageVo,
    pub request_window: TenantRequestWindowUsageVo,
    pub auxiliary: TenantAuxiliaryUsageVo,
}

impl From<ServiceTenantUsageVo> for TenantUsageVo {
    fn from(value: ServiceTenantUsageVo) -> Self {
        Self {
            tenant_id: value.tenant_id,
            calculated_at: value.calculated_at,
            users: value.users.into(),
            roles: value.roles.into(),
            storage: value.storage.into(),
            request_window: value.request_window.into(),
            auxiliary: value.auxiliary.into(),
        }
    }
}

/// 平台租户分页与详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantCapacityVo {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    /// 对外统一使用 `enabled` 或 `disabled`。
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
    pub expiration_status: String,
    /// 调用者没有 `tenant:usage:list` 权限时为 `None`。
    pub capacity_status: Option<String>,
    /// 调用者没有 `tenant:usage:list` 权限时为 `None`。
    pub usage: Option<TenantUsageVo>,
}

impl From<ServiceTenantCapacityVo> for TenantCapacityVo {
    fn from(value: ServiceTenantCapacityVo) -> Self {
        let tenant = value.tenant;
        Self {
            tenant_id: tenant.tenant_id,
            name: tenant.name,
            domain: tenant.domain,
            status: tenant.status,
            expire_at: tenant.expire_at,
            max_users: tenant.max_users,
            max_roles: tenant.max_roles,
            max_storage_mb: tenant.max_storage_mb,
            max_requests_per_min: tenant.max_requests_per_min,
            expiration_status: value.expiration_status,
            capacity_status: value.capacity_status,
            usage: value.usage.map(Into::into),
        }
    }
}
