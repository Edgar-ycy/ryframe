use ryframe_application::{
    TenantDataPoolStats, TenantDataTargetAccess, TenantDataTargetFuture, TenantDataTargetHealth,
    TenantDataTargetMetadata, TenantDataTargetPort,
};

use crate::{
    TenantDatabaseRouter, TenantDatabaseTargetHealthStatus, migration::TENANT_DATA_CATALOG,
};

use super::map_error;

impl TenantDataTargetPort for TenantDatabaseRouter {
    fn contains(&self, target_key: &str) -> bool {
        self.targets().contains(target_key)
    }

    fn is_dedicated(&self, target_key: &str) -> Option<bool> {
        self.targets().target_is_dedicated(target_key)
    }

    fn mode_code(&self, target_key: &str) -> Option<&'static str> {
        self.targets().target_mode_code(target_key)
    }

    fn kind_code(&self, target_key: &str) -> Option<&'static str> {
        self.targets().target_kind_code(target_key)
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
                .targets()
                .metadata()
                .await
                .into_iter()
                .map(|metadata| {
                    let mode = metadata.mode_code().into();
                    let kind = metadata.kind_code().into();
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
                        health: map_health(metadata.health),
                        last_verified_at: metadata.last_verified_at.map(Into::into),
                    }
                })
                .collect())
        })
    }

    fn pool_stats(&self) -> TenantDataTargetFuture<'_, TenantDataPoolStats> {
        Box::pin(async move {
            let stats = self.targets().pool_stats().await;
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
            self.verify_target_now_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_error)
        })
    }

    fn validate_catalog<'a>(
        &'a self,
        target_key: &'a str,
    ) -> TenantDataTargetFuture<'a, TenantDataTargetAccess> {
        Box::pin(async move {
            self.open_target_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map(|target| TenantDataTargetAccess {
                    dedicated: target.is_dedicated(),
                })
                .map_err(map_error)
        })
    }

    fn is_occupied<'a>(&'a self, target_key: &'a str) -> TenantDataTargetFuture<'a, bool> {
        Box::pin(async move {
            self.target_occupancy_for_catalog(target_key, &TENANT_DATA_CATALOG)
                .await
                .map(|occupancy| occupancy.is_some())
                .map_err(map_error)
        })
    }

    fn tenant_is_empty<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: &'a str,
    ) -> TenantDataTargetFuture<'a, bool> {
        Box::pin(async move {
            self.tenant_is_empty_on_target_for_catalog(target_key, tenant_id, &TENANT_DATA_CATALOG)
                .await
                .map_err(map_error)
        })
    }
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
