use std::sync::Arc;

use ryframe_application::ports::{
    jobs::BackgroundJobTransaction,
    tenant_data::{
        CreateTenantDataMigrationRecord, TenantDataBackupPointRecord,
        TenantDataMigrationItemRecord, TenantDataMigrationPersistencePort,
        TenantDataMigrationRecord, TenantDataMigrationTransaction, TenantDataPlacementRecord,
        TenantMigrationContextRecord, TenantOperationLeaseRecord,
    },
};
use ryframe_db::{
    ControlDatabaseCluster, TenantDataRepository, TenantOperationLeaseRepository, TenantRepository,
    application_ports::transaction::DatabasePortTransaction, database_utc_now,
};
use sea_orm::TransactionTrait;

mod mappers;

use mappers::{
    database_error, map_backup_point, map_create_migration, map_lease, validated_backup_query,
};
pub use mappers::{
    map_item, map_item_model, map_migration, map_migration_model, map_placement,
    map_placement_model,
};

struct TenantDataMigrationPersistence {
    database: ControlDatabaseCluster,
}

struct TenantDataMigrationWorkUnit {
    transaction: DatabasePortTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantDataMigrationPersistencePort> {
    Arc::new(TenantDataMigrationPersistence { database })
}

#[async_trait::async_trait]
impl TenantDataMigrationPersistencePort for TenantDataMigrationPersistence {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        database_utc_now(self.database.write()).await
    }

    async fn occupied_target_keys<'a>(
        &'a self,
        configured_target_keys: &'a [String],
    ) -> ryframe_kernel::AppResult<std::collections::HashSet<String>> {
        TenantDataRepository
            .occupied_target_keys(self.database.write(), configured_target_keys)
            .await
    }

    async fn placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantDataPlacementRecord>> {
        TenantDataRepository
            .placement(self.database.write(), tenant_id)
            .await
            .map(|record| record.map(map_placement))
    }

    async fn migration(
        &self,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantDataMigrationRecord>> {
        TenantDataRepository
            .migration(self.database.write(), id)
            .await
            .map(|record| record.map(map_migration))
    }

    async fn migrations_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<TenantDataMigrationRecord>> {
        TenantDataRepository
            .migrations_for_tenant(self.database.write(), tenant_id, limit)
            .await
            .map(|records| records.into_iter().map(map_migration).collect())
    }

    async fn recoverable_migrations(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<TenantDataMigrationRecord>> {
        TenantDataRepository
            .recoverable_migrations(self.database.write(), after_id, limit)
            .await
            .map(|records| records.into_iter().map(map_migration).collect())
    }

    async fn migration_by_create_key<'a>(
        &'a self,
        key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantDataMigrationRecord>> {
        TenantDataRepository
            .migration_by_create_key(self.database.write(), key_hash)
            .await
            .map(|record| record.map(map_migration))
    }

    async fn active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantDataMigrationRecord>> {
        TenantDataRepository
            .active_migration_for_tenant(self.database.write(), tenant_id)
            .await
            .map(|record| record.map(map_migration))
    }

    async fn items(
        &self,
        migration_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<TenantDataMigrationItemRecord>> {
        TenantDataRepository
            .items(self.database.write(), migration_id)
            .await
            .map(|records| records.into_iter().map(map_item).collect())
    }

    async fn insert_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationItemRecord> {
        TenantDataRepository
            .insert_item(self.database.write(), map_item_model(item))
            .await
            .map(map_item)
    }

    async fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationItemRecord> {
        TenantDataRepository
            .save_item(self.database.write(), map_item_model(item))
            .await
            .map(map_item)
    }

    async fn backup_points_for_target<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: Option<&'a str>,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<TenantDataBackupPointRecord>> {
        TenantDataRepository
            .backup_points_for_target(self.database.write(), target_key, tenant_id, limit)
            .await
            .map(|records| records.into_iter().map(map_backup_point).collect())
    }

    async fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantDataRepository
            .validated_backup_for_destination(
                self.database.write(),
                validated_backup_query(migration, not_before, now),
            )
            .await
            .map(|record| record.is_some())
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn TenantDataMigrationTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(TenantDataMigrationWorkUnit {
            transaction: transaction.into(),
        }) as Box<dyn TenantDataMigrationTransaction>)
    }
}

#[async_trait::async_trait]
impl TenantDataMigrationTransaction for TenantDataMigrationWorkUnit {
    fn background_jobs(&self) -> &dyn BackgroundJobTransaction {
        &self.transaction
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        database_utc_now(&self.transaction).await
    }

    async fn acquire_lease(
        &self,
        lease: TenantOperationLeaseRecord,
    ) -> ryframe_kernel::AppResult<()> {
        TenantOperationLeaseRepository
            .acquire_in_txn(&self.transaction, map_lease(lease))
            .await
            .map(|_| ())
    }

    async fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantOperationLeaseRepository
            .renew_in_txn(&self.transaction, tenant_id, owner_token, expires_at)
            .await
    }

    async fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantOperationLeaseRepository
            .release_in_txn(&self.transaction, tenant_id, owner_token)
            .await
    }

    async fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> ryframe_kernel::AppResult<TenantMigrationContextRecord> {
        TenantOperationLeaseRepository
            .lock_tenant_and_validate_in_txn(&self.transaction, tenant_id, owner_token)
            .await
            .map(|tenant| TenantMigrationContextRecord {
                authorization_epoch: tenant.authorization_epoch,
            })
    }

    async fn increment_runtime_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i64> {
        TenantRepository
            .increment_runtime_epoch_in_txn(&self.transaction, tenant_id)
            .await
    }

    async fn lock_placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantDataPlacementRecord> {
        TenantDataRepository
            .lock_placement_in_txn(&self.transaction, tenant_id)
            .await
            .map(map_placement)
    }

    async fn save_placement(
        &self,
        placement: TenantDataPlacementRecord,
    ) -> ryframe_kernel::AppResult<TenantDataPlacementRecord> {
        TenantDataRepository
            .save_placement_in_txn(&self.transaction, map_placement_model(placement))
            .await
            .map(map_placement)
    }

    async fn lock_migration(
        &self,
        id: i64,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationRecord> {
        TenantDataRepository
            .lock_migration_in_txn(&self.transaction, id)
            .await
            .map(map_migration)
    }

    async fn lock_active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantDataMigrationRecord>> {
        TenantDataRepository
            .lock_active_migration_for_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|record| record.map(map_migration))
    }

    async fn insert_migration(
        &self,
        command: CreateTenantDataMigrationRecord,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationRecord> {
        TenantDataRepository
            .insert_migration_in_txn(&self.transaction, map_create_migration(command))
            .await
            .map(map_migration)
    }

    async fn save_migration(
        &self,
        migration: TenantDataMigrationRecord,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationRecord> {
        TenantDataRepository
            .save_migration_in_txn(&self.transaction, map_migration_model(migration))
            .await
            .map(map_migration)
    }

    async fn lock_item(&self, id: i64) -> ryframe_kernel::AppResult<TenantDataMigrationItemRecord> {
        TenantDataRepository
            .lock_item_in_txn(&self.transaction, id)
            .await
            .map(map_item)
    }

    async fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> ryframe_kernel::AppResult<TenantDataMigrationItemRecord> {
        TenantDataRepository
            .save_item(&self.transaction, map_item_model(item))
            .await
            .map(map_item)
    }

    async fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<bool> {
        TenantDataRepository
            .validated_backup_for_destination(
                &self.transaction,
                validated_backup_query(migration, not_before, now),
            )
            .await
            .map(|record| record.is_some())
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for TenantDataMigrationWorkUnit {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        let _ = audit_mode;
        self.transaction.commit().await.map_err(database_error)
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}
