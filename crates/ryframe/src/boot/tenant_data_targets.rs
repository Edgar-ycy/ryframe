use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_application::{
    TenantDataPoolStats, TenantDataTargetAccess, TenantDataTargetFuture, TenantDataTargetHealth,
    TenantDataTargetMetadata, TenantDataTargetPort,
};
use ryframe_tenant_db::{
    TenantDatabaseRouter, TenantDatabaseTargetHealthStatus, migration::TENANT_DATA_CATALOG,
};

use super::tenant_data::map_error as map_tenant_data_error;

struct TenantDataTargetBridge {
    router: Arc<TenantDatabaseRouter>,
}

impl TenantDataTargetPort for TenantDataTargetBridge {
    fn contains(&self, target_key: &str) -> bool {
        self.router.targets().contains(target_key)
    }

    fn is_dedicated(&self, target_key: &str) -> Option<bool> {
        self.router.targets().target_is_dedicated(target_key)
    }

    fn mode_code(&self, target_key: &str) -> Option<&'static str> {
        self.router.targets().target_mode_code(target_key)
    }

    fn kind_code(&self, target_key: &str) -> Option<&'static str> {
        self.router.targets().target_kind_code(target_key)
    }

    fn catalog_fingerprint(&self) -> String {
        TENANT_DATA_CATALOG.schema_fingerprint()
    }

    fn catalog_table_count(&self) -> usize {
        TENANT_DATA_CATALOG.tables().len()
    }

    fn metadata(&self) -> TenantDataTargetFuture<'_, Vec<TenantDataTargetMetadata>> {
        Box::pin(async move {
            Ok(self
                .router
                .targets()
                .metadata()
                .await
                .into_iter()
                .map(|metadata| {
                    let mode = metadata.mode_code().into();
                    let kind = metadata.kind_code().into();
                    let health = map_health(metadata.health);
                    TenantDataTargetMetadata {
                        key: metadata.key,
                        display_name: metadata.display_name,
                        region: metadata.region,
                        mode,
                        kind,
                        connected: metadata.connected,
                        pool_max_connections: metadata.pool_max_connections,
                        active_leases: metadata.active_leases,
                        schema_fingerprint: metadata.schema_fingerprint,
                        health,
                        last_verified_at: metadata.last_verified_at.map(DateTime::<Utc>::from),
                    }
                })
                .collect())
        })
    }

    fn pool_stats(&self) -> TenantDataTargetFuture<'_, TenantDataPoolStats> {
        Box::pin(async move {
            let stats = self.router.targets().pool_stats().await;
            Ok(TenantDataPoolStats {
                reserved_connections: stats.reserved_connections,
                max_total_connections: stats.max_total_connections,
                open_targets: stats.open_targets,
                opening_targets: stats.opening_targets,
            })
        })
    }

    fn verify_now<'a>(&'a self, target_key: &'a str) -> TenantDataTargetFuture<'a, ()> {
        Box::pin(async move {
            self.router
                .verify_target_now_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn validate_catalog<'a>(
        &'a self,
        target_key: &'a str,
    ) -> TenantDataTargetFuture<'a, TenantDataTargetAccess> {
        Box::pin(async move {
            self.router
                .open_target_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map(|target| TenantDataTargetAccess {
                    dedicated: target.is_dedicated(),
                })
                .map_err(map_tenant_data_error)
        })
    }

    fn is_occupied<'a>(&'a self, target_key: &'a str) -> TenantDataTargetFuture<'a, bool> {
        Box::pin(async move {
            self.router
                .target_occupancy_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map(|occupancy| occupancy.is_some())
                .map_err(map_tenant_data_error)
        })
    }

    fn tenant_is_empty<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: &'a str,
    ) -> TenantDataTargetFuture<'a, bool> {
        Box::pin(async move {
            self.router
                .tenant_is_empty_on_target_for_catalog(target_key, tenant_id, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_tenant_data_error)
        })
    }
}

pub fn port(router: Arc<TenantDatabaseRouter>) -> Arc<dyn TenantDataTargetPort> {
    Arc::new(TenantDataTargetBridge { router })
}

const fn map_health(health: TenantDatabaseTargetHealthStatus) -> TenantDataTargetHealth {
    match health {
        TenantDatabaseTargetHealthStatus::Unknown => TenantDataTargetHealth::Unknown,
        TenantDatabaseTargetHealthStatus::Verified => TenantDataTargetHealth::Verified,
        TenantDatabaseTargetHealthStatus::Unavailable => TenantDataTargetHealth::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_health_mapping_is_complete() {
        assert_eq!(
            map_health(TenantDatabaseTargetHealthStatus::Unknown),
            TenantDataTargetHealth::Unknown
        );
        assert_eq!(
            map_health(TenantDatabaseTargetHealthStatus::Verified),
            TenantDataTargetHealth::Verified
        );
        assert_eq!(
            map_health(TenantDatabaseTargetHealthStatus::Unavailable),
            TenantDataTargetHealth::Unavailable
        );
    }
}
