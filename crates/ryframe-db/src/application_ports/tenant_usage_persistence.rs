use std::{collections::BTreeMap, sync::Arc};

use crate::{
    ControlDatabaseCluster, ReadConsistency, TenantRepository, TenantUsageAggregate,
    TenantUsagePageFilter, TenantUsageRepository, entities::tenant,
};

use ryframe_application::{
    PersistenceFuture, TenantCapacityRecord, TenantUsageAggregateRecord, TenantUsageFilter,
    TenantUsagePersistencePort,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantUsagePersistencePort> {
    Arc::new(DatabaseTenantUsagePersistence { database })
}

struct DatabaseTenantUsagePersistence {
    database: ControlDatabaseCluster,
}

impl TenantUsagePersistencePort for DatabaseTenantUsagePersistence {
    fn page<'a>(
        &'a self,
        filter: TenantUsageFilter<'a>,
        page: &'a ryframe_kernel::ValidatedPageQuery,
        calculated_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ryframe_kernel::PageResult<TenantCapacityRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let result = TenantUsageRepository
                .page(
                    &database,
                    TenantUsagePageFilter {
                        tenant_id: filter.tenant_id,
                        name: filter.name,
                        status: filter.status,
                        expiration_status: filter.expiration_status,
                        capacity_status: filter.capacity_status,
                    },
                    page,
                    calculated_at,
                )
                .await?;
            Ok(ryframe_kernel::PageResult {
                records: result.records.into_iter().map(to_tenant_record).collect(),
                total: result.total,
                page: result.page,
                page_size: result.page_size,
            })
        })
    }

    fn find<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantCapacityRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            Ok(TenantRepository
                .find_by_tenant_id(&database, tenant_id)
                .await?
                .map(to_tenant_record))
        })
    }

    fn aggregate<'a>(
        &'a self,
        tenant_ids: &'a [String],
    ) -> PersistenceFuture<'a, BTreeMap<String, TenantUsageAggregateRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            Ok(TenantUsageRepository
                .aggregate_for_tenants(&database, tenant_ids)
                .await?
                .into_iter()
                .map(to_aggregate_entry)
                .collect())
        })
    }
}

fn to_tenant_record(tenant: tenant::Model) -> TenantCapacityRecord {
    TenantCapacityRecord {
        tenant_id: tenant.tenant_id,
        name: tenant.name,
        domain: tenant.domain,
        status: tenant.status,
        expire_at: tenant.expire_at,
        max_users: tenant.max_users,
        max_roles: tenant.max_roles,
        max_storage_mb: tenant.max_storage_mb,
        max_requests_per_min: tenant.max_requests_per_min,
    }
}

fn to_aggregate_entry(aggregate: TenantUsageAggregate) -> (String, TenantUsageAggregateRecord) {
    (
        aggregate.tenant_id,
        TenantUsageAggregateRecord {
            users: aggregate.users,
            roles: aggregate.roles,
            storage_bytes: aggregate.storage_bytes,
            pending_jobs: aggregate.pending_jobs,
            running_jobs: aggregate.running_jobs,
            dead_jobs: aggregate.dead_jobs,
            enabled_schedules: aggregate.enabled_schedules,
            active_user_imports: aggregate.active_user_imports,
        },
    )
}
