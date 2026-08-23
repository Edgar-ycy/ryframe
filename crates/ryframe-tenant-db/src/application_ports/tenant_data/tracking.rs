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
    ControlDatabaseCluster, CreateTenantDataMigration, TenantDataRepository,
    TenantOperationLeaseRepository, TenantRepository, ValidatedTenantDataBackup,
    application_ports::transaction::DatabasePortTransaction,
    database_utc_now,
    entities::tenant::{
        data_backup_point as tenant_data_backup_point, data_migration as tenant_data_migration,
        data_migration_item as tenant_data_migration_item, data_placement as tenant_data_placement,
        operation_lease as tenant_operation_lease,
    },
};
use ryframe_kernel::AppError;
use sea_orm::TransactionTrait;

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

pub fn map_migration(model: tenant_data_migration::Model) -> TenantDataMigrationRecord {
    TenantDataMigrationRecord {
        id: model.id,
        tenant_id: model.tenant_id,
        source_target_key: model.source_target_key,
        target_key: model.target_key,
        source_target_mode: model.source_target_mode,
        source_target_kind: model.source_target_kind,
        target_target_mode: model.target_target_mode,
        target_target_kind: model.target_target_kind,
        source_generation: model.source_generation,
        source_switch_token: model.source_switch_token,
        target_generation: model.target_generation,
        source_schema_fingerprint: model.source_schema_fingerprint,
        target_schema_fingerprint: model.target_schema_fingerprint,
        plan_hash: model.plan_hash,
        create_idempotency_key_hash: model.create_idempotency_key_hash,
        cancel_idempotency_key_hash: model.cancel_idempotency_key_hash,
        finalize_idempotency_key_hash: model.finalize_idempotency_key_hash,
        state: model.state,
        switch_token: model.switch_token,
        operator_id: model.operator_id,
        cancelled_by: model.cancelled_by,
        finalized_by: model.finalized_by,
        background_job_id: model.background_job_id,
        retention_hours: model.retention_hours,
        error_code: model.error_code,
        error_detail: model.error_detail,
        prechecked_at: model.prechecked_at,
        queued_at: model.queued_at,
        quiesced_at: model.quiesced_at,
        frozen_at: model.frozen_at,
        copy_started_at: model.copy_started_at,
        copy_completed_at: model.copy_completed_at,
        verified_at: model.verified_at,
        cut_over_at: model.cut_over_at,
        activated_at: model.activated_at,
        succeeded_at: model.succeeded_at,
        retention_until: model.retention_until,
        cancel_requested_at: model.cancel_requested_at,
        finalize_requested_at: model.finalize_requested_at,
        cleanup_ready_at: model.cleanup_ready_at,
        finalized_at: model.finalized_at,
        failed_at: model.failed_at,
        cancelled_at: model.cancelled_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub fn map_migration_model(record: TenantDataMigrationRecord) -> tenant_data_migration::Model {
    tenant_data_migration::Model {
        id: record.id,
        tenant_id: record.tenant_id,
        source_target_key: record.source_target_key,
        target_key: record.target_key,
        source_target_mode: record.source_target_mode,
        source_target_kind: record.source_target_kind,
        target_target_mode: record.target_target_mode,
        target_target_kind: record.target_target_kind,
        source_generation: record.source_generation,
        source_switch_token: record.source_switch_token,
        target_generation: record.target_generation,
        source_schema_fingerprint: record.source_schema_fingerprint,
        target_schema_fingerprint: record.target_schema_fingerprint,
        plan_hash: record.plan_hash,
        create_idempotency_key_hash: record.create_idempotency_key_hash,
        cancel_idempotency_key_hash: record.cancel_idempotency_key_hash,
        finalize_idempotency_key_hash: record.finalize_idempotency_key_hash,
        state: record.state,
        switch_token: record.switch_token,
        operator_id: record.operator_id,
        cancelled_by: record.cancelled_by,
        finalized_by: record.finalized_by,
        background_job_id: record.background_job_id,
        retention_hours: record.retention_hours,
        error_code: record.error_code,
        error_detail: record.error_detail,
        prechecked_at: record.prechecked_at,
        queued_at: record.queued_at,
        quiesced_at: record.quiesced_at,
        frozen_at: record.frozen_at,
        copy_started_at: record.copy_started_at,
        copy_completed_at: record.copy_completed_at,
        verified_at: record.verified_at,
        cut_over_at: record.cut_over_at,
        activated_at: record.activated_at,
        succeeded_at: record.succeeded_at,
        retention_until: record.retention_until,
        cancel_requested_at: record.cancel_requested_at,
        finalize_requested_at: record.finalize_requested_at,
        cleanup_ready_at: record.cleanup_ready_at,
        finalized_at: record.finalized_at,
        failed_at: record.failed_at,
        cancelled_at: record.cancelled_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

pub fn map_item(model: tenant_data_migration_item::Model) -> TenantDataMigrationItemRecord {
    TenantDataMigrationItemRecord {
        id: model.id,
        migration_id: model.migration_id,
        table_name: model.table_name,
        copy_order: model.copy_order,
        state: model.state,
        cursor_json: model.cursor_json,
        source_row_count: model.source_row_count,
        target_row_count: model.target_row_count,
        source_digest: model.source_digest,
        target_digest: model.target_digest,
        error_code: model.error_code,
        error_detail: model.error_detail,
        copy_started_at: model.copy_started_at,
        copied_at: model.copied_at,
        verified_at: model.verified_at,
        cleanup_state: model.cleanup_state,
        cleanup_row_count: model.cleanup_row_count,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub fn map_item_model(record: TenantDataMigrationItemRecord) -> tenant_data_migration_item::Model {
    tenant_data_migration_item::Model {
        id: record.id,
        migration_id: record.migration_id,
        table_name: record.table_name,
        copy_order: record.copy_order,
        state: record.state,
        cursor_json: record.cursor_json,
        source_row_count: record.source_row_count,
        target_row_count: record.target_row_count,
        source_digest: record.source_digest,
        target_digest: record.target_digest,
        error_code: record.error_code,
        error_detail: record.error_detail,
        copy_started_at: record.copy_started_at,
        copied_at: record.copied_at,
        verified_at: record.verified_at,
        cleanup_state: record.cleanup_state,
        cleanup_row_count: record.cleanup_row_count,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

pub fn map_placement(model: tenant_data_placement::Model) -> TenantDataPlacementRecord {
    TenantDataPlacementRecord {
        tenant_id: model.tenant_id,
        current_target_key: model.current_target_key,
        placement_generation: model.placement_generation,
        state: model.state,
        switch_token: model.switch_token,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub fn map_placement_model(record: TenantDataPlacementRecord) -> tenant_data_placement::Model {
    tenant_data_placement::Model {
        tenant_id: record.tenant_id,
        current_target_key: record.current_target_key,
        placement_generation: record.placement_generation,
        state: record.state,
        switch_token: record.switch_token,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn map_backup_point(model: tenant_data_backup_point::Model) -> TenantDataBackupPointRecord {
    TenantDataBackupPointRecord {
        id: model.id,
        scope: model.scope,
        tenant_id: model.tenant_id,
        target_key: model.target_key,
        placement_generation: model.placement_generation,
        schema_fingerprint: model.schema_fingerprint,
        provider_ref: model.provider_ref,
        captured_at: model.captured_at,
        checksum: model.checksum,
        validation_status: model.validation_status,
        validation_detail: model.validation_detail,
        retention_until: model.retention_until,
        expires_at: model.expires_at,
        last_restore_drill_at: model.last_restore_drill_at,
        created_by: model.created_by,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn map_create_migration(command: CreateTenantDataMigrationRecord) -> CreateTenantDataMigration {
    CreateTenantDataMigration {
        id: command.id,
        tenant_id: command.tenant_id,
        source_target_key: command.source_target_key,
        target_key: command.target_key,
        source_target_mode: command.source_target_mode,
        source_target_kind: command.source_target_kind,
        target_target_mode: command.target_target_mode,
        target_target_kind: command.target_target_kind,
        source_generation: command.source_generation,
        source_switch_token: command.source_switch_token,
        target_generation: command.target_generation,
        source_schema_fingerprint: command.source_schema_fingerprint,
        target_schema_fingerprint: command.target_schema_fingerprint,
        plan_hash: command.plan_hash,
        create_idempotency_key_hash: command.create_idempotency_key_hash,
        switch_token: command.switch_token,
        operator_id: command.operator_id,
        retention_hours: command.retention_hours,
        now: command.now,
    }
}

fn validated_backup_query(
    migration: &TenantDataMigrationRecord,
    not_before: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> ValidatedTenantDataBackup<'_> {
    ValidatedTenantDataBackup {
        tenant_id: &migration.tenant_id,
        target_key: &migration.target_key,
        target_mode: &migration.target_target_mode,
        target_generation: migration.target_generation,
        schema_fingerprint: &migration.target_schema_fingerprint,
        not_before,
        now,
    }
}

fn map_lease(record: TenantOperationLeaseRecord) -> tenant_operation_lease::Model {
    tenant_operation_lease::Model {
        tenant_id: record.tenant_id,
        owner_token: record.owner_token,
        operation: record.operation,
        resource_type: record.resource_type,
        resource_id: record.resource_id,
        expires_at: record.expires_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
