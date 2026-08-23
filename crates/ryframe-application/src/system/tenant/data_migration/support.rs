use ryframe_kernel::{ActorContext, AppError, AppResult};
use sha2::{Digest, Sha256};

use crate::ports::tenant_data::{
    TenantDataBackupPointRecord, TenantDataMigrationItemRecord, TenantDataMigrationRecord,
    TenantDataPlacementRecord, TenantDataTargetHealth, TenantDataTargetMetadata,
};

use super::{
    BackupPointView, CreateMigrationCommand, DataPlacementView, DataTargetSummary,
    MigrationItemView, MigrationView, RETENTION_HOURS,
};

pub(super) fn ensure_platform_actor(actor: &ActorContext) -> AppResult<()> {
    if actor.tenant_id != "system" {
        return Err(AppError::Authorization(
            "数据放置平台仅允许 system 租户访问".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_migration_tenant(tenant_id: &str) -> AppResult<()> {
    // 数据迁移由 system 租户的控制面操作员发起，目标租户天然不同于请求主体。
    // 此处只校验目标标识；目标租户可能与发起请求的控制面租户不同。
    // 非 system 租户的迁移列表、预览和创建操作。
    ryframe_kernel::TenantId::parse(tenant_id)?;
    if tenant_id == "system" {
        return Err(AppError::Validation("system 租户禁止迁移业务数据".into()));
    }
    Ok(())
}

pub(super) fn validate_idempotency_key(key: &str) -> AppResult<()> {
    if key != key.trim() || key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 1–128 位可打印字符".into(),
        ));
    }
    Ok(())
}

pub(super) fn ensure_same_create_request(
    migration: &TenantDataMigrationRecord,
    command: &CreateMigrationCommand,
) -> AppResult<()> {
    if migration.target_key == command.target_key
        && migration.plan_hash == command.plan_hash
        && migration.source_generation == command.expected_placement_generation
    {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "Idempotency-Key 已用于不同的数据迁移请求".into(),
        ))
    }
}

pub(super) fn create_blocker_error(blockers: &[String]) -> AppError {
    if blockers
        .iter()
        .any(|blocker| blocker == "placement_not_active")
    {
        return AppError::TenantDataMaintenance("租户业务数据当前不可迁移".into(), 5);
    }
    if blockers.iter().any(|blocker| {
        matches!(
            blocker.as_str(),
            "tenant_operation_in_progress" | "dedicated_target_occupied"
        )
    }) {
        return AppError::TenantOperationConflict("租户或专属目标正在执行其他操作".into());
    }
    if blockers.iter().any(|blocker| {
        matches!(
            blocker.as_str(),
            "source_target_not_registered"
                | "source_target_unavailable"
                | "target_unavailable"
                | "target_occupancy_unavailable"
                | "target_empty_check_unavailable"
        )
    }) {
        return AppError::TenantDataTargetUnavailable("租户数据目标当前不可用".into(), 5);
    }
    AppError::Validation(format!("租户数据迁移不可执行: {}", blockers.join(",")))
}

pub(super) fn target_summary(metadata: TenantDataTargetMetadata) -> DataTargetSummary {
    let health = match metadata.health {
        TenantDataTargetHealth::Unknown => "unknown",
        TenantDataTargetHealth::Verified => "verified",
        TenantDataTargetHealth::Unavailable => "unavailable",
    };
    let mut reasons = Vec::new();
    if metadata.health != TenantDataTargetHealth::Verified {
        reasons.push(if metadata.health == TenantDataTargetHealth::Unavailable {
            "target_unavailable".into()
        } else {
            "target_not_verified".into()
        });
    }
    DataTargetSummary {
        key: metadata.key,
        display_name: metadata.display_name,
        mode: metadata.mode,
        kind: metadata.kind,
        region: metadata.region,
        health: health.into(),
        schema_fingerprint: metadata.schema_fingerprint,
        connected: metadata.connected,
        pool_max_connections: metadata.pool_max_connections,
        active_leases: metadata.active_leases,
        eligible: reasons.is_empty(),
        reasons,
    }
}

pub(super) struct MigrationPlanHashInput<'a> {
    pub(super) tenant_id: &'a str,
    pub(super) source_target_key: &'a str,
    pub(super) target_key: &'a str,
    pub(super) source_generation: i64,
    pub(super) target_generation: i64,
    pub(super) source_mode: Option<&'static str>,
    pub(super) source_kind: Option<&'static str>,
    pub(super) target_mode: Option<&'static str>,
    pub(super) target_kind: Option<&'static str>,
    pub(super) schema_fingerprint: &'a str,
}

pub(super) fn migration_plan_hash(input: MigrationPlanHashInput<'_>) -> String {
    let MigrationPlanHashInput {
        tenant_id,
        source_target_key,
        target_key,
        source_generation,
        target_generation,
        source_mode,
        source_kind,
        target_mode,
        target_kind,
        schema_fingerprint,
    } = input;
    let source_mode = source_mode.unwrap_or("unknown");
    let source_kind = source_kind.unwrap_or("unknown");
    let target_mode = target_mode.unwrap_or("unknown");
    let target_kind = target_kind.unwrap_or("unknown");
    sha256_hex(&format!(
        "ryframe:tenant-data:plan:v1|tenant={tenant_id}|source={source_target_key}|target={target_key}|source_generation={source_generation}|target_generation={target_generation}|schema={schema_fingerprint}|source_mode={source_mode}|source_kind={source_kind}|target_mode={target_mode}|target_kind={target_kind}|migration_mode=stop_write|retention_hours={RETENTION_HOURS}",
    ))
}

pub(super) fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

impl From<TenantDataBackupPointRecord> for BackupPointView {
    fn from(model: TenantDataBackupPointRecord) -> Self {
        Self {
            id: model.id.to_string(),
            scope: model.scope,
            tenant_id: model.tenant_id,
            target_key: model.target_key,
            placement_generation: model.placement_generation.map(|value| value.to_string()),
            schema_fingerprint: model.schema_fingerprint,
            captured_at: model.captured_at,
            checksum: model.checksum,
            validation_status: model.validation_status,
            retention_until: model.retention_until,
            expires_at: model.expires_at,
            last_restore_drill_at: model.last_restore_drill_at,
        }
    }
}

impl From<TenantDataPlacementRecord> for DataPlacementView {
    fn from(model: TenantDataPlacementRecord) -> Self {
        Self {
            tenant_id: model.tenant_id,
            current_target_key: model.current_target_key,
            placement_generation: model.placement_generation.to_string(),
            state: model.state,
            updated_at: model.updated_at,
        }
    }
}

impl MigrationView {
    pub(super) fn from_models(
        migration: TenantDataMigrationRecord,
        items: Vec<TenantDataMigrationItemRecord>,
        can_cancel: bool,
        can_finalize: bool,
        action_reasons: Vec<String>,
    ) -> Self {
        let cancel_requested = migration.cancel_requested_at.is_some();
        let finalize_requested = migration.finalize_requested_at.is_some();
        Self {
            id: migration.id.to_string(),
            tenant_id: migration.tenant_id,
            source_target_key: migration.source_target_key,
            target_target_key: migration.target_key,
            source_generation: migration.source_generation.to_string(),
            target_generation: migration.target_generation.to_string(),
            source_schema_fingerprint: migration.source_schema_fingerprint,
            target_schema_fingerprint: migration.target_schema_fingerprint,
            plan_hash: migration.plan_hash,
            state: migration.state,
            operator_id: migration.operator_id.to_string(),
            retention_hours: migration.retention_hours,
            error_code: migration.error_code,
            prechecked_at: migration.prechecked_at,
            queued_at: migration.queued_at,
            quiesced_at: migration.quiesced_at,
            frozen_at: migration.frozen_at,
            copy_started_at: migration.copy_started_at,
            copy_completed_at: migration.copy_completed_at,
            verified_at: migration.verified_at,
            cut_over_at: migration.cut_over_at,
            activated_at: migration.activated_at,
            succeeded_at: migration.succeeded_at,
            retention_until: migration.retention_until,
            finalized_at: migration.finalized_at,
            failed_at: migration.failed_at,
            cancelled_at: migration.cancelled_at,
            created_at: migration.created_at,
            updated_at: migration.updated_at,
            can_cancel,
            can_finalize,
            cancel_requested,
            finalize_requested,
            action_reasons,
            items: items.into_iter().map(MigrationItemView::from).collect(),
        }
    }
}

impl From<TenantDataMigrationItemRecord> for MigrationItemView {
    fn from(model: TenantDataMigrationItemRecord) -> Self {
        Self {
            id: model.id.to_string(),
            table_name: model.table_name,
            copy_order: model.copy_order,
            state: model.state,
            cursor: model.cursor_json,
            source_row_count: model.source_row_count.map(|value| value.to_string()),
            target_row_count: model.target_row_count.map(|value| value.to_string()),
            source_digest: model.source_digest,
            target_digest: model.target_digest,
            error_code: model.error_code,
            cleanup_state: model.cleanup_state,
            cleanup_row_count: model.cleanup_row_count.to_string(),
        }
    }
}
