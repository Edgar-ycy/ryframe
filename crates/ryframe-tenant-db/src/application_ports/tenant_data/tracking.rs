use std::sync::Arc;

use ryframe_application::{
    PersistenceFuture,
    ports::{
        jobs::BackgroundJobTransaction,
        tenant_data::{
            CreateTenantDataMigrationRecord, TenantDataBackupPointRecord,
            TenantDataMigrationItemRecord, TenantDataMigrationPersistencePort,
            TenantDataMigrationRecord, TenantDataMigrationTransaction, TenantDataPlacementRecord,
            TenantMigrationContextRecord, TenantOperationLeaseRecord,
        },
    },
};
use ryframe_db::{
    ControlDatabaseCluster, CreateTenantDataMigration, TenantDataRepository,
    TenantOperationLeaseRepository, TenantRepository, ValidatedTenantDataBackup,
    application_ports::transaction::DatabasePortTransaction,
    entities::{
        tenant_data_backup_point, tenant_data_migration, tenant_data_migration_item,
        tenant_data_placement, tenant_operation_lease,
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

impl TenantDataMigrationPersistencePort for TenantDataMigrationPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn occupied_target_keys<'a>(
        &'a self,
        configured_target_keys: &'a [String],
    ) -> PersistenceFuture<'a, std::collections::HashSet<String>> {
        Box::pin(async move {
            TenantDataRepository
                .occupied_target_keys(self.database.write(), configured_target_keys)
                .await
        })
    }

    fn placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataPlacementRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .placement(self.database.write(), tenant_id)
                .await
                .map(|record| record.map(map_placement))
        })
    }

    fn migration(&self, id: i64) -> PersistenceFuture<'_, Option<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .migration(self.database.write(), id)
                .await
                .map(|record| record.map(map_migration))
        })
    }

    fn migrations_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .migrations_for_tenant(self.database.write(), tenant_id, limit)
                .await
                .map(|records| records.into_iter().map(map_migration).collect())
        })
    }

    fn recoverable_migrations(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .recoverable_migrations(self.database.write(), after_id, limit)
                .await
                .map(|records| records.into_iter().map(map_migration).collect())
        })
    }

    fn migration_by_create_key<'a>(
        &'a self,
        key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .migration_by_create_key(self.database.write(), key_hash)
                .await
                .map(|record| record.map(map_migration))
        })
    }

    fn active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .active_migration_for_tenant(self.database.write(), tenant_id)
                .await
                .map(|record| record.map(map_migration))
        })
    }

    fn items(
        &self,
        migration_id: i64,
    ) -> PersistenceFuture<'_, Vec<TenantDataMigrationItemRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .items(self.database.write(), migration_id)
                .await
                .map(|records| records.into_iter().map(map_item).collect())
        })
    }

    fn insert_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord> {
        Box::pin(async move {
            TenantDataRepository
                .insert_item(self.database.write(), map_item_model(item))
                .await
                .map(map_item)
        })
    }

    fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord> {
        Box::pin(async move {
            TenantDataRepository
                .save_item(self.database.write(), map_item_model(item))
                .await
                .map(map_item)
        })
    }

    fn backup_points_for_target<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: Option<&'a str>,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<TenantDataBackupPointRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .backup_points_for_target(self.database.write(), target_key, tenant_id, limit)
                .await
                .map(|records| records.into_iter().map(map_backup_point).collect())
        })
    }

    fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantDataRepository
                .validated_backup_for_destination(
                    self.database.write(),
                    validated_backup_query(migration, not_before, now),
                )
                .await
                .map(|record| record.is_some())
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn TenantDataMigrationTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(TenantDataMigrationWorkUnit {
                transaction: transaction.into(),
            }) as Box<dyn TenantDataMigrationTransaction>)
        })
    }
}

impl TenantDataMigrationTransaction for TenantDataMigrationWorkUnit {
    fn background_jobs(&self) -> &dyn BackgroundJobTransaction {
        &self.transaction
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .database_utc_now(&self.transaction)
                .await
        })
    }

    fn acquire_lease(&self, lease: TenantOperationLeaseRecord) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .acquire_in_txn(&self.transaction, map_lease(lease))
                .await
                .map(|_| ())
        })
    }

    fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .renew_in_txn(&self.transaction, tenant_id, owner_token, expires_at)
                .await
        })
    }

    fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .release_in_txn(&self.transaction, tenant_id, owner_token)
                .await
        })
    }

    fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> PersistenceFuture<'a, TenantMigrationContextRecord> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .lock_tenant_and_validate_in_txn(&self.transaction, tenant_id, owner_token)
                .await
                .map(|tenant| TenantMigrationContextRecord {
                    authorization_epoch: tenant.authorization_epoch,
                })
        })
    }

    fn increment_runtime_epoch<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            TenantRepository
                .increment_runtime_epoch_in_txn(&self.transaction, tenant_id)
                .await
        })
    }

    fn lock_placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, TenantDataPlacementRecord> {
        Box::pin(async move {
            TenantDataRepository
                .lock_placement_in_txn(&self.transaction, tenant_id)
                .await
                .map(map_placement)
        })
    }

    fn save_placement(
        &self,
        placement: TenantDataPlacementRecord,
    ) -> PersistenceFuture<'_, TenantDataPlacementRecord> {
        Box::pin(async move {
            TenantDataRepository
                .save_placement_in_txn(&self.transaction, map_placement_model(placement))
                .await
                .map(map_placement)
        })
    }

    fn lock_migration(&self, id: i64) -> PersistenceFuture<'_, TenantDataMigrationRecord> {
        Box::pin(async move {
            TenantDataRepository
                .lock_migration_in_txn(&self.transaction, id)
                .await
                .map(map_migration)
        })
    }

    fn lock_active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>> {
        Box::pin(async move {
            TenantDataRepository
                .lock_active_migration_for_tenant_in_txn(&self.transaction, tenant_id)
                .await
                .map(|record| record.map(map_migration))
        })
    }

    fn insert_migration(
        &self,
        command: CreateTenantDataMigrationRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationRecord> {
        Box::pin(async move {
            TenantDataRepository
                .insert_migration_in_txn(&self.transaction, map_create_migration(command))
                .await
                .map(map_migration)
        })
    }

    fn save_migration(
        &self,
        migration: TenantDataMigrationRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationRecord> {
        Box::pin(async move {
            TenantDataRepository
                .save_migration_in_txn(&self.transaction, map_migration_model(migration))
                .await
                .map(map_migration)
        })
    }

    fn lock_item(&self, id: i64) -> PersistenceFuture<'_, TenantDataMigrationItemRecord> {
        Box::pin(async move {
            TenantDataRepository
                .lock_item_in_txn(&self.transaction, id)
                .await
                .map(map_item)
        })
    }

    fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord> {
        Box::pin(async move {
            TenantDataRepository
                .save_item(&self.transaction, map_item_model(item))
                .await
                .map(map_item)
        })
    }

    fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            TenantDataRepository
                .validated_backup_for_destination(
                    &self.transaction,
                    validated_backup_query(migration, not_before, now),
                )
                .await
                .map(|record| record.is_some())
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn map_migration(model: tenant_data_migration::Model) -> TenantDataMigrationRecord {
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

fn map_migration_model(record: TenantDataMigrationRecord) -> tenant_data_migration::Model {
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

fn map_item(model: tenant_data_migration_item::Model) -> TenantDataMigrationItemRecord {
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

fn map_item_model(record: TenantDataMigrationItemRecord) -> tenant_data_migration_item::Model {
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

fn map_placement(model: tenant_data_placement::Model) -> TenantDataPlacementRecord {
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

fn map_placement_model(record: TenantDataPlacementRecord) -> tenant_data_placement::Model {
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

fn validated_backup_query<'a>(
    migration: &'a TenantDataMigrationRecord,
    not_before: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> ValidatedTenantDataBackup<'a> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_mapping_preserves_every_field() {
        let now = "2026-08-21T01:02:03Z".parse().unwrap();
        let later = "2026-08-22T04:05:06Z".parse().unwrap();
        let model = tenant_data_migration::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            source_target_key: "source".into(),
            target_key: "target".into(),
            source_target_mode: "shared".into(),
            source_target_kind: "control".into(),
            target_target_mode: "dedicated".into(),
            target_target_kind: "external".into(),
            source_generation: 2,
            source_switch_token: "source-token".into(),
            target_generation: 3,
            source_schema_fingerprint: "source-schema".into(),
            target_schema_fingerprint: "target-schema".into(),
            plan_hash: "plan".into(),
            create_idempotency_key_hash: "create".into(),
            cancel_idempotency_key_hash: Some("cancel".into()),
            finalize_idempotency_key_hash: Some("finalize".into()),
            state: tenant_data_migration::Model::STATE_COPYING.into(),
            switch_token: "switch".into(),
            operator_id: 4,
            cancelled_by: Some(5),
            finalized_by: Some(6),
            background_job_id: Some(7),
            retention_hours: 168,
            error_code: Some("error".into()),
            error_detail: Some("detail".into()),
            prechecked_at: Some(now),
            queued_at: Some(now),
            quiesced_at: Some(now),
            frozen_at: Some(now),
            copy_started_at: Some(now),
            copy_completed_at: Some(later),
            verified_at: Some(later),
            cut_over_at: Some(later),
            activated_at: Some(later),
            succeeded_at: Some(later),
            retention_until: Some(later),
            cancel_requested_at: Some(now),
            finalize_requested_at: Some(later),
            cleanup_ready_at: Some(later),
            finalized_at: Some(later),
            failed_at: Some(later),
            cancelled_at: Some(later),
            created_at: now,
            updated_at: later,
        };

        assert_eq!(map_migration_model(map_migration(model.clone())), model);
    }

    #[test]
    fn migration_item_and_placement_mapping_preserve_every_field() {
        let now = "2026-08-21T01:02:03Z".parse().unwrap();
        let later = "2026-08-22T04:05:06Z".parse().unwrap();
        let item = tenant_data_migration_item::Model {
            id: 11,
            migration_id: 12,
            table_name: "sys_user".into(),
            copy_order: 13,
            state: tenant_data_migration_item::Model::STATE_VERIFIED.into(),
            cursor_json: Some(sea_orm::prelude::Json::Array(vec![
                sea_orm::prelude::Json::String("a".into()),
                sea_orm::prelude::Json::String("b".into()),
            ])),
            source_row_count: Some(14),
            target_row_count: Some(15),
            source_digest: Some("source".into()),
            target_digest: Some("target".into()),
            error_code: Some("error".into()),
            error_detail: Some("detail".into()),
            copy_started_at: Some(now),
            copied_at: Some(later),
            verified_at: Some(later),
            cleanup_state: tenant_data_migration_item::Model::CLEANUP_CLEANING.into(),
            cleanup_row_count: 16,
            created_at: now,
            updated_at: later,
        };
        let placement = tenant_data_placement::Model {
            tenant_id: "tenant-a".into(),
            current_target_key: "target".into(),
            placement_generation: 17,
            state: tenant_data_placement::Model::STATE_MAINTENANCE.into(),
            switch_token: "switch".into(),
            created_at: now,
            updated_at: later,
        };

        assert_eq!(map_item_model(map_item(item.clone())), item);
        assert_eq!(
            map_placement_model(map_placement(placement.clone())),
            placement
        );
    }
}
