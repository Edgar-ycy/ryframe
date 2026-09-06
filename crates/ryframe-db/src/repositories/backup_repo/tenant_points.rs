use crate::{
    DbResultExt,
    entities::tenant::data_backup_point,
    repositories::{RegisterTenantDataBackupPoint, TenantDataRepository},
};
use ryframe_application::ports::backup::{BackupDatabaseKind, BackupRecord, DatabaseBackup};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait};

pub(super) fn provider_ref(backup_id: &str, target: &str) -> String {
    format!("backup-set:{backup_id}:{target}")
}

pub(super) async fn save<C: ConnectionTrait>(db: &C, record: &BackupRecord) -> AppResult<()> {
    let repository = TenantDataRepository;
    let manifest = &record.manifest;
    for target in &manifest.databases {
        if target.kind == BackupDatabaseKind::Control
            || (!target.shared && target.placements.is_empty())
        {
            continue;
        }
        let reference = provider_ref(&manifest.id, &target.key);
        if repository
            .backup_by_provider_ref(db, &reference)
            .await?
            .is_some()
        {
            let (existing, expected) = lock_projection(db, record, target).await?;
            if existing.validation_status == expected.validation_status
                && existing.validation_detail == expected.validation_detail
            {
                continue;
            }
            let updated_at = existing.updated_at.max(record.checked_at);
            let mut model = data_backup_point::ActiveModel::from(existing);
            model.validation_status = Set(expected.validation_status);
            model.validation_detail = Set(expected.validation_detail);
            model.updated_at = Set(updated_at);
            model.update(db).await.db()?;
        } else {
            let id = crate::next_id()?;
            repository
                .insert_backup(db, registration_command(record, target, reference, id))
                .await?;
        }
    }
    Ok(())
}

pub(super) async fn lock_projection<C: ConnectionTrait>(
    db: &C,
    record: &BackupRecord,
    target: &DatabaseBackup,
) -> AppResult<(data_backup_point::Model, RegisterTenantDataBackupPoint)> {
    let reference = provider_ref(&record.manifest.id, &target.key);
    let repository = TenantDataRepository;
    let existing = repository
        .lock_backup_by_provider_ref(db, &reference)
        .await?
        .ok_or_else(|| AppError::Database("备份点权威投影不可读".into()))?;
    let expected = registration_command(record, target, reference, existing.id);
    validate_projection(&existing, &expected)?;
    Ok((existing, expected))
}

fn registration_command(
    record: &BackupRecord,
    target: &DatabaseBackup,
    reference: String,
    id: i64,
) -> RegisterTenantDataBackupPoint {
    let placement = (!target.shared)
        .then(|| target.placements.first())
        .flatten();
    RegisterTenantDataBackupPoint {
        id,
        scope: if target.shared { "shard" } else { "tenant" }.into(),
        tenant_id: placement.map(|item| item.tenant_id.clone()),
        target_key: target.key.clone(),
        placement_generation: placement.map(|item| item.generation),
        schema_fingerprint: record.manifest.tenant_schema_fingerprint.clone(),
        provider_ref: reference,
        captured_at: record.manifest.captured_at,
        checksum: Some(record.manifest_hash.clone()),
        validation_status: if record.valid { "valid" } else { "invalid" }.into(),
        validation_detail: record.failure.clone(),
        retention_until: record.manifest.retention_until,
        expires_at: Some(record.manifest.retention_until),
        created_by: None,
        now: record.checked_at,
    }
}

fn validate_projection(
    existing: &data_backup_point::Model,
    expected: &RegisterTenantDataBackupPoint,
) -> AppResult<()> {
    let matches = existing.scope == expected.scope
        && existing.tenant_id == expected.tenant_id
        && existing.target_key == expected.target_key
        && existing.placement_generation == expected.placement_generation
        && existing.schema_fingerprint == expected.schema_fingerprint
        && existing.provider_ref == expected.provider_ref
        && existing.captured_at == expected.captured_at
        && existing.checksum == expected.checksum
        && existing.retention_until == expected.retention_until
        && existing.expires_at == expected.expires_at
        && existing.created_by == expected.created_by;
    if !matches {
        return Err(AppError::Conflict(
            "备份点引用已绑定到不同的不可变投影".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration};
    use ryframe_application::ports::backup::{BackupManifest, BackupPlacement};

    fn record(valid: bool, failure: Option<&str>) -> BackupRecord {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        BackupRecord {
            manifest: BackupManifest {
                id: "backup-1".into(),
                scope_id: "source".into(),
                source_sha: "a".repeat(40),
                quiesced_at: now - Duration::hours(2),
                captured_at: now - Duration::hours(1),
                completed_at: now - Duration::minutes(1),
                retention_until: now + Duration::days(7),
                control_schema_fingerprint: "b".repeat(16),
                tenant_schema_fingerprint: "c".repeat(64),
                databases: vec![],
                objects: vec![],
                artifacts: vec![],
            },
            manifest_hash: "d".repeat(64),
            valid,
            checked_at: now,
            failure: failure.map(str::to_owned),
        }
    }

    fn target(shared: bool) -> DatabaseBackup {
        DatabaseBackup {
            key: "target-1".into(),
            kind: BackupDatabaseKind::Tenant,
            server_uuid: "server".into(),
            database: "tenant_db".into(),
            shared,
            placements: (!shared)
                .then(|| BackupPlacement {
                    tenant_id: "tenant-1".into(),
                    generation: 3,
                    switch_token: "switch-1".into(),
                })
                .into_iter()
                .collect(),
            tables: vec![],
        }
    }

    #[test]
    fn first_registration_preserves_validation_and_placement_projection() {
        let invalid = registration_command(
            &record(false, Some("校验和错误")),
            &target(true),
            "backup-set:backup-1:target-1".into(),
            42,
        );
        assert_eq!(invalid.validation_status, "invalid");
        assert_eq!(invalid.validation_detail.as_deref(), Some("校验和错误"));
        assert_eq!((invalid.scope.as_str(), invalid.tenant_id), ("shard", None));

        let valid = registration_command(
            &record(true, None),
            &target(false),
            "backup-set:backup-1:target-1".into(),
            43,
        );
        assert_eq!(valid.validation_status, "valid");
        assert_eq!(valid.validation_detail, None);
        assert_eq!(valid.scope, "tenant");
        assert_eq!(valid.tenant_id.as_deref(), Some("tenant-1"));
        assert_eq!(valid.placement_generation, Some(3));
    }

    #[test]
    fn existing_projection_must_match_all_immutable_fields() {
        let command = registration_command(
            &record(true, None),
            &target(true),
            "backup-set:backup-1:target-1".into(),
            42,
        );
        let mut model = data_backup_point::Model {
            id: command.id,
            scope: command.scope.clone(),
            tenant_id: command.tenant_id.clone(),
            target_key: command.target_key.clone(),
            placement_generation: command.placement_generation,
            schema_fingerprint: command.schema_fingerprint.clone(),
            provider_ref: command.provider_ref.clone(),
            captured_at: command.captured_at,
            checksum: command.checksum.clone(),
            validation_status: command.validation_status.clone(),
            validation_detail: command.validation_detail.clone(),
            retention_until: command.retention_until,
            expires_at: command.expires_at,
            last_restore_drill_at: None,
            created_by: command.created_by,
            created_at: command.now,
            updated_at: command.now,
        };
        assert!(validate_projection(&model, &command).is_ok());
        model.target_key = "other-target".into();
        assert!(validate_projection(&model, &command).is_err());
    }
}
