use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

#[derive(Clone, Debug)]
pub struct TenantCapacityRecord {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TenantUsageAggregateRecord {
    pub users: u64,
    pub roles: u64,
    pub storage_bytes: u64,
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub dead_jobs: u64,
    pub enabled_schedules: u64,
    pub active_user_imports: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TenantUsageFilter<'a> {
    pub tenant_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub expiration_status: Option<&'a str>,
    pub capacity_status: Option<&'a str>,
}

pub trait TenantUsagePersistencePort: Send + Sync {
    fn page<'a>(
        &'a self,
        filter: TenantUsageFilter<'a>,
        page: &'a ValidatedPageQuery,
        calculated_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, PageResult<TenantCapacityRecord>>;

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantCapacityRecord>>;

    fn aggregate<'a>(
        &'a self,
        tenant_ids: &'a [String],
    ) -> PersistenceFuture<'a, BTreeMap<String, TenantUsageAggregateRecord>>;
}
