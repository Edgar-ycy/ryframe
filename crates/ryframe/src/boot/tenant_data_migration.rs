use std::sync::Arc;

use ryframe_application::{
    TenantDataCatalogTable, TenantDataCleanupOwnership as ApplicationCleanupOwnership,
    TenantDataFence, TenantDataMigrationFuture, TenantDataMigrationPort, map_tenant_data_error,
};
use ryframe_kernel::{AppError, AppResult};
use ryframe_tenant_db::{
    TenantDataCleanupBatch, TenantDataCleanupOwnership, TenantDatabaseRouter,
    migration::{TENANT_DATA_CATALOG, TenantDataTableDescriptor},
};

struct TenantDataMigrationBridge {
    router: Arc<TenantDatabaseRouter>,
}

impl TenantDataMigrationPort for TenantDataMigrationBridge {
    fn catalog_tables(&self) -> Vec<TenantDataCatalogTable> {
        TENANT_DATA_CATALOG
            .tables()
            .iter()
            .map(|table| TenantDataCatalogTable {
                name: table.table,
                copy_order: table.copy_order,
            })
            .collect()
    }

    fn prepare_target<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .prepare_migration_target_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn clear_prepared_target<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .clear_prepared_target_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn freeze_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .freeze_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn activate_fence<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .activate_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn assert_frozen_fence<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .assert_frozen_fence_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn cleanup_ownership<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
    ) -> TenantDataMigrationFuture<'a, ApplicationCleanupOwnership> {
        Box::pin(async move {
            self.router
                .cleanup_ownership_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map(map_cleanup_ownership)
                .map_err(map_tenant_data_error)
        })
    }

    fn delete_rows_batch<'a>(
        &'a self,
        fence: TenantDataFence<'a>,
        table: &'a str,
        batch_size: u32,
    ) -> TenantDataMigrationFuture<'a, u64> {
        Box::pin(async move {
            let descriptor = catalog_table(table)?;
            self.router
                .delete_tenant_rows_batch_for_catalog(
                    TenantDataCleanupBatch {
                        tenant_id: fence.tenant_id,
                        target_key: fence.target_key,
                        placement_generation: fence.generation,
                        switch_token: fence.switch_token,
                        descriptor,
                        batch_size,
                    },
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }

    fn finish_cleanup<'a>(&'a self, fence: TenantDataFence<'a>) -> TenantDataMigrationFuture<'a> {
        Box::pin(async move {
            self.router
                .finish_tenant_cleanup_for_catalog(
                    fence.tenant_id,
                    fence.target_key,
                    fence.generation,
                    fence.switch_token,
                    &TENANT_DATA_CATALOG,
                )
                .await
                .map_err(map_tenant_data_error)
        })
    }
}

pub fn port(router: Arc<TenantDatabaseRouter>) -> Arc<dyn TenantDataMigrationPort> {
    Arc::new(TenantDataMigrationBridge { router })
}

fn catalog_table(table: &str) -> AppResult<&'static TenantDataTableDescriptor> {
    TENANT_DATA_CATALOG
        .tables()
        .iter()
        .find(|descriptor| descriptor.table == table)
        .ok_or_else(|| AppError::Validation(format!("未知租户数据表: {table}")))
}

const fn map_cleanup_ownership(
    ownership: TenantDataCleanupOwnership,
) -> ApplicationCleanupOwnership {
    match ownership {
        TenantDataCleanupOwnership::OwnedFrozen => ApplicationCleanupOwnership::OwnedFrozen,
        TenantDataCleanupOwnership::AlreadyClean => ApplicationCleanupOwnership::AlreadyClean,
        TenantDataCleanupOwnership::NotOwned => ApplicationCleanupOwnership::NotOwned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_ownership_mapping_is_complete() {
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::OwnedFrozen),
            ApplicationCleanupOwnership::OwnedFrozen
        );
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::AlreadyClean),
            ApplicationCleanupOwnership::AlreadyClean
        );
        assert_eq!(
            map_cleanup_ownership(TenantDataCleanupOwnership::NotOwned),
            ApplicationCleanupOwnership::NotOwned
        );
    }

    #[test]
    fn catalog_lookup_rejects_unknown_table() {
        assert!(catalog_table("unknown_table").is_err());
    }
}
