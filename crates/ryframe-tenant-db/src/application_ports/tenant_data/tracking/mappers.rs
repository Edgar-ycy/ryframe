use ryframe_application::ports::tenant_data::{
    CreateTenantDataMigrationRecord, TenantDataBackupPointRecord, TenantDataMigrationItemRecord,
    TenantDataMigrationRecord, TenantDataPlacementRecord, TenantOperationLeaseRecord,
};
use ryframe_db::{
    CreateTenantDataMigration, ValidatedTenantDataBackup,
    entities::tenant::{
        data_backup_point as tenant_data_backup_point, data_migration as tenant_data_migration,
        data_migration_item as tenant_data_migration_item, data_placement as tenant_data_placement,
        operation_lease as tenant_operation_lease,
    },
};

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

pub(super) fn map_backup_point(
    model: tenant_data_backup_point::Model,
) -> TenantDataBackupPointRecord {
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

pub(super) fn map_create_migration(
    command: CreateTenantDataMigrationRecord,
) -> CreateTenantDataMigration {
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

pub(super) fn validated_backup_query(
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

pub(super) fn map_lease(record: TenantOperationLeaseRecord) -> tenant_operation_lease::Model {
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
