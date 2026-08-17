use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_core::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    ControlDatabaseCluster, ReadConsistency, TenantRepository, TenantUsageAggregate,
    TenantUsagePageFilter, TenantUsageRepository, entities::tenant,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_middleware::RateLimiter;
use serde::Serialize;

use super::tenant_service::TenantVo;

const SYSTEM_TENANT_ID: &str = "system";
const TENANT_RATE_LIMIT_WINDOW_SECS: u64 = 60;

#[derive(Clone, Debug, Default)]
pub struct TenantUsagePageParams {
    pub tenant_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub expiration_status: Option<String>,
    pub capacity_status: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct QuotaUsage {
    pub used: u64,
    /// `None` 表示该资源不受配额限制。
    pub limit: Option<u64>,
    /// 使用率基点，10000 表示 100%；无限制时为 `None`。
    pub percentage_basis_points: Option<u32>,
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RequestWindowUsage {
    pub current: Option<u64>,
    /// `None` 表示租户请求限流未启用。
    pub limit: Option<u64>,
    pub percentage_basis_points: Option<u32>,
    pub remaining_secs: Option<u64>,
    /// 请求窗口状态独立于租户资源 `capacity_status`，Redis 故障时只降级为 `unknown`。
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TenantAuxiliaryUsage {
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub dead_jobs: u64,
    pub enabled_schedules: u64,
    pub active_user_imports: u64,
    pub cron_enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TenantUsageVo {
    pub tenant_id: String,
    pub calculated_at: DateTime<Utc>,
    pub users: QuotaUsage,
    pub roles: QuotaUsage,
    pub storage: QuotaUsage,
    pub request_window: RequestWindowUsage,
    pub auxiliary: TenantAuxiliaryUsage,
}

#[derive(Debug, Serialize)]
pub struct TenantCapacityVo {
    pub tenant: TenantVo,
    pub expiration_status: String,
    /// 仅由主库中的用户、角色与存储用量推导，当前请求限流窗口不参与该状态。
    pub capacity_status: Option<String>,
    pub usage: Option<TenantUsageVo>,
}

pub struct TenantUsageService {
    db: ControlDatabaseCluster,
    tenant_repo: TenantRepository,
    usage_repo: TenantUsageRepository,
    rate_limiter: Arc<RateLimiter>,
    rate_limit_enabled: bool,
    scheduler_enabled: bool,
}

impl TenantUsageService {
    pub fn new(
        db: ControlDatabaseCluster,
        rate_limiter: Arc<RateLimiter>,
        rate_limit_enabled: bool,
        scheduler_enabled: bool,
    ) -> Self {
        Self {
            db,
            tenant_repo: TenantRepository,
            usage_repo: TenantUsageRepository,
            rate_limiter,
            rate_limit_enabled,
            scheduler_enabled,
        }
    }

    pub async fn page(
        &self,
        actor: &ActorContext,
        params: TenantUsagePageParams,
        page: &ValidatedPageQuery,
        include_usage: bool,
    ) -> AppResult<PageResult<TenantCapacityVo>> {
        ensure_system_tenant(actor)?;
        let params = normalize_page_params(params);
        validate_page_params(&params)?;
        if !include_usage && params.capacity_status.is_some() {
            return Err(AppError::Authorization(
                "筛选租户容量状态需要租户用量查看权限".into(),
            ));
        }
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let calculated_at = Utc::now();
        let result = self
            .usage_repo
            .page(
                &db,
                TenantUsagePageFilter {
                    tenant_id: params.tenant_id.as_deref(),
                    name: params.name.as_deref(),
                    status: tenant_status_db(params.status.as_deref()),
                    expiration_status: params.expiration_status.as_deref(),
                    capacity_status: params.capacity_status.as_deref(),
                },
                page,
                calculated_at,
            )
            .await?;
        let tenants = result.records;
        let usage = if include_usage {
            self.load_usage_map(&db, &tenants).await?
        } else {
            BTreeMap::new()
        };
        let records = tenants
            .into_iter()
            .map(|tenant| {
                let tenant_id = tenant.tenant_id.clone();
                tenant_capacity_vo(tenant, usage.get(&tenant_id).cloned(), calculated_at)
            })
            .collect();
        Ok(PageResult::new(records, result.total, page))
    }

    pub async fn detail(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        include_usage: bool,
    ) -> AppResult<TenantCapacityVo> {
        ensure_system_tenant(actor)?;
        ryframe_core::validate_tenant_identifier(tenant_id)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let tenant = self
            .tenant_repo
            .find_by_tenant_id(&db, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let usage = if include_usage {
            self.load_usage_map(&db, std::slice::from_ref(&tenant))
                .await?
                .remove(tenant_id)
        } else {
            None
        };
        Ok(tenant_capacity_vo(tenant, usage, Utc::now()))
    }

    pub async fn usage(&self, actor: &ActorContext, tenant_id: &str) -> AppResult<TenantUsageVo> {
        ensure_system_tenant(actor)?;
        ryframe_core::validate_tenant_identifier(tenant_id)?;
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let tenant = self
            .tenant_repo
            .find_by_tenant_id(&db, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        self.load_usage_map(&db, std::slice::from_ref(&tenant))
            .await?
            .remove(tenant_id)
            .ok_or_else(|| AppError::Database("租户容量聚合缺少目标租户".into()))
    }

    async fn load_usage_map(
        &self,
        db: &sea_orm::DatabaseConnection,
        tenants: &[tenant::Model],
    ) -> AppResult<BTreeMap<String, TenantUsageVo>> {
        if tenants.is_empty() {
            return Ok(BTreeMap::new());
        }
        let tenant_ids = tenants
            .iter()
            .map(|tenant| tenant.tenant_id.clone())
            .collect::<Vec<_>>();
        let aggregates = self
            .usage_repo
            .aggregate_for_tenants(db, &tenant_ids)
            .await?
            .into_iter()
            .map(|usage| (usage.tenant_id.clone(), usage))
            .collect::<BTreeMap<_, _>>();
        let request_windows = self.request_window_map(tenants).await;
        let calculated_at = Utc::now();
        Ok(tenants
            .iter()
            .map(|tenant| {
                let aggregate = aggregates
                    .get(&tenant.tenant_id)
                    .cloned()
                    .unwrap_or_else(|| TenantUsageAggregate {
                        tenant_id: tenant.tenant_id.clone(),
                        ..Default::default()
                    });
                let request_window = request_windows
                    .get(&tenant.tenant_id)
                    .cloned()
                    .unwrap_or_else(|| unknown_request_window(tenant));
                (
                    tenant.tenant_id.clone(),
                    tenant_usage_vo(
                        tenant,
                        aggregate,
                        request_window,
                        self.scheduler_enabled,
                        calculated_at,
                    ),
                )
            })
            .collect())
    }

    async fn request_window_map(
        &self,
        tenants: &[tenant::Model],
    ) -> BTreeMap<String, RequestWindowUsage> {
        if !self.rate_limit_enabled {
            return tenants
                .iter()
                .map(|tenant| (tenant.tenant_id.clone(), unlimited_request_window()))
                .collect();
        }
        let limited = tenants
            .iter()
            .filter(|tenant| tenant.max_requests_per_min > 0)
            .collect::<Vec<_>>();
        let mut windows = tenants
            .iter()
            .filter(|tenant| tenant.max_requests_per_min == 0)
            .map(|tenant| (tenant.tenant_id.clone(), unlimited_request_window()))
            .collect::<BTreeMap<_, _>>();
        if limited.is_empty() {
            return windows;
        }
        let keys = limited
            .iter()
            .map(|tenant| RateLimiter::tenant_key(&tenant.tenant_id))
            .collect::<Vec<_>>();
        // 每个租户额度可能不同，快照中的 limit 只承载默认值，响应使用租户表中的真实额度。
        match self.rate_limiter.snapshot_many(&keys, 1).await {
            Ok(snapshots) => {
                for (tenant, snapshot) in limited.into_iter().zip(snapshots) {
                    let limit = u64::try_from(tenant.max_requests_per_min).unwrap_or_default();
                    windows.insert(
                        tenant.tenant_id.clone(),
                        RequestWindowUsage {
                            current: Some(snapshot.current),
                            limit: Some(limit),
                            percentage_basis_points: percentage_basis_points(
                                snapshot.current,
                                limit,
                            ),
                            remaining_secs: Some(
                                snapshot.remaining_secs.min(TENANT_RATE_LIMIT_WINDOW_SECS),
                            ),
                            status: quota_status(snapshot.current, limit).to_owned(),
                        },
                    );
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "租户请求限流窗口读取失败，容量接口局部降级");
                for tenant in limited {
                    windows.insert(
                        tenant.tenant_id.clone(),
                        RequestWindowUsage {
                            current: None,
                            limit: Some(
                                u64::try_from(tenant.max_requests_per_min).unwrap_or_default(),
                            ),
                            percentage_basis_points: None,
                            remaining_secs: None,
                            status: "unknown".to_owned(),
                        },
                    );
                }
            }
        }
        windows
    }
}

fn tenant_capacity_vo(
    tenant: tenant::Model,
    usage: Option<TenantUsageVo>,
    now: DateTime<Utc>,
) -> TenantCapacityVo {
    let capacity_status = usage.as_ref().map(capacity_status);
    TenantCapacityVo {
        expiration_status: expiration_status(&tenant, now).to_owned(),
        capacity_status,
        tenant: TenantVo::from(tenant),
        usage,
    }
}

fn tenant_usage_vo(
    tenant: &tenant::Model,
    aggregate: TenantUsageAggregate,
    request_window: RequestWindowUsage,
    scheduler_enabled: bool,
    calculated_at: DateTime<Utc>,
) -> TenantUsageVo {
    let user_limit = u64::try_from(tenant.max_users).unwrap_or_default();
    let role_limit = u64::try_from(tenant.max_roles).unwrap_or_default();
    let storage_limit = u64::try_from(tenant.max_storage_mb)
        .unwrap_or_default()
        .saturating_mul(1024 * 1024);
    TenantUsageVo {
        tenant_id: tenant.tenant_id.clone(),
        calculated_at,
        users: quota_usage(aggregate.users, user_limit),
        roles: quota_usage(aggregate.roles, role_limit),
        storage: quota_usage(aggregate.storage_bytes, storage_limit),
        request_window,
        auxiliary: TenantAuxiliaryUsage {
            pending_jobs: aggregate.pending_jobs,
            running_jobs: aggregate.running_jobs,
            dead_jobs: aggregate.dead_jobs,
            enabled_schedules: aggregate.enabled_schedules,
            active_user_imports: aggregate.active_user_imports,
            cron_enabled: scheduler_enabled,
        },
    }
}

fn quota_usage(used: u64, limit: u64) -> QuotaUsage {
    QuotaUsage {
        used,
        limit: (limit > 0).then_some(limit),
        percentage_basis_points: percentage_basis_points(used, limit),
        status: quota_status(used, limit).to_owned(),
    }
}

fn quota_status(used: u64, limit: u64) -> &'static str {
    if limit == 0 {
        return "unlimited";
    }
    if used >= limit {
        return "exceeded";
    }
    let scaled = u128::from(used).saturating_mul(100);
    let limit = u128::from(limit);
    if scaled >= limit.saturating_mul(90) {
        "critical"
    } else if scaled >= limit.saturating_mul(80) {
        "warning"
    } else {
        "normal"
    }
}

fn percentage_basis_points(used: u64, limit: u64) -> Option<u32> {
    if limit == 0 {
        return None;
    }
    let basis_points = u128::from(used)
        .saturating_mul(10_000)
        .checked_div(u128::from(limit))?
        .min(u128::from(u32::MAX));
    u32::try_from(basis_points).ok()
}

fn capacity_status(usage: &TenantUsageVo) -> String {
    let statuses = [
        usage.users.status.as_str(),
        usage.roles.status.as_str(),
        usage.storage.status.as_str(),
    ];
    for status in ["exceeded", "critical", "warning", "normal"] {
        if statuses.contains(&status) {
            return status.to_owned();
        }
    }
    "unlimited".to_owned()
}

fn expiration_status(tenant: &tenant::Model, now: DateTime<Utc>) -> &'static str {
    match tenant.expire_at {
        None => "never",
        Some(expire_at) if expire_at <= now => "expired",
        Some(expire_at) if expire_at <= now + chrono::Duration::days(30) => "expiring",
        Some(_) => "active",
    }
}

fn unknown_request_window(tenant: &tenant::Model) -> RequestWindowUsage {
    RequestWindowUsage {
        current: None,
        limit: (tenant.max_requests_per_min > 0)
            .then(|| u64::try_from(tenant.max_requests_per_min).unwrap_or_default()),
        percentage_basis_points: None,
        remaining_secs: None,
        status: if tenant.max_requests_per_min == 0 {
            "unlimited".to_owned()
        } else {
            "unknown".to_owned()
        },
    }
}

fn unlimited_request_window() -> RequestWindowUsage {
    RequestWindowUsage {
        current: Some(0),
        limit: None,
        percentage_basis_points: None,
        remaining_secs: Some(0),
        status: "unlimited".to_owned(),
    }
}

fn validate_page_params(params: &TenantUsagePageParams) -> AppResult<()> {
    if params
        .status
        .as_deref()
        .is_some_and(|value| !matches!(value, "enabled" | "disabled"))
    {
        return Err(AppError::Validation("租户状态筛选值无效".into()));
    }
    if params
        .expiration_status
        .as_deref()
        .is_some_and(|value| !matches!(value, "active" | "expiring" | "expired" | "never"))
    {
        return Err(AppError::Validation("租户到期状态筛选值无效".into()));
    }
    if params.capacity_status.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "normal" | "warning" | "critical" | "exceeded" | "unlimited" | "unknown"
        )
    }) {
        return Err(AppError::Validation("租户容量状态筛选值无效".into()));
    }
    Ok(())
}

fn tenant_status_db(status: Option<&str>) -> Option<&'static str> {
    match status {
        Some("enabled") => Some(tenant::Model::STATUS_NORMAL),
        Some("disabled") => Some(tenant::Model::STATUS_DISABLED),
        _ => None,
    }
}

fn normalize_page_params(mut params: TenantUsagePageParams) -> TenantUsagePageParams {
    params.tenant_id = normalize_optional(params.tenant_id);
    params.name = normalize_optional(params.name);
    params.status = normalize_optional(params.status);
    params.expiration_status = normalize_optional(params.expiration_status);
    params.capacity_status = normalize_optional(params.capacity_status);
    params
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn ensure_system_tenant(actor: &ActorContext) -> AppResult<()> {
    crate::validated_tenant_id(actor)?;
    if actor.tenant_id != SYSTEM_TENANT_ID {
        return Err(AppError::Authorization("仅系统租户可以查看租户容量".into()));
    }
    Ok(())
}
