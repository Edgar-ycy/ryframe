use crate::entities::tenant::data_backup_point;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue::Set;

#[derive(Clone, Debug)]
pub struct CreateTenantDataMigration {
    pub id: i64,
    pub tenant_id: String,
    pub source_target_key: String,
    pub target_key: String,
    pub source_target_mode: String,
    pub source_target_kind: String,
    pub target_target_mode: String,
    pub target_target_kind: String,
    pub source_generation: i64,
    pub source_switch_token: String,
    pub target_generation: i64,
    pub source_schema_fingerprint: String,
    pub target_schema_fingerprint: String,
    pub plan_hash: String,
    pub create_idempotency_key_hash: String,
    pub switch_token: String,
    pub operator_id: i64,
    pub retention_hours: i32,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct RegisterTenantDataBackupPoint {
    pub id: i64,
    pub scope: String,
    pub tenant_id: Option<String>,
    pub target_key: String,
    pub placement_generation: Option<i64>,
    pub schema_fingerprint: String,
    pub provider_ref: String,
    pub captured_at: DateTime<Utc>,
    pub checksum: Option<String>,
    pub validation_status: String,
    pub validation_detail: Option<String>,
    pub retention_until: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<i64>,
    pub now: DateTime<Utc>,
}

pub(super) fn backup_active_model(
    command: RegisterTenantDataBackupPoint,
) -> data_backup_point::ActiveModel {
    data_backup_point::ActiveModel {
        id: Set(command.id),
        scope: Set(command.scope),
        tenant_id: Set(command.tenant_id),
        target_key: Set(command.target_key),
        placement_generation: Set(command.placement_generation),
        schema_fingerprint: Set(command.schema_fingerprint),
        provider_ref: Set(command.provider_ref),
        captured_at: Set(command.captured_at),
        checksum: Set(command.checksum),
        validation_status: Set(command.validation_status),
        validation_detail: Set(command.validation_detail),
        retention_until: Set(command.retention_until),
        expires_at: Set(command.expires_at),
        last_restore_drill_at: Set(None),
        created_by: Set(command.created_by),
        created_at: Set(command.now),
        updated_at: Set(command.now),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ValidatedTenantDataBackup<'a> {
    pub tenant_id: &'a str,
    pub target_key: &'a str,
    pub target_mode: &'a str,
    pub target_generation: i64,
    pub schema_fingerprint: &'a str,
    pub not_before: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(detail: Option<&str>) -> RegisterTenantDataBackupPoint {
        let now = DateTime::from_timestamp(1_700_000_000, 123_000_000).unwrap();
        RegisterTenantDataBackupPoint {
            id: 42,
            scope: "shard".into(),
            tenant_id: None,
            target_key: "shared-control".into(),
            placement_generation: None,
            schema_fingerprint: "schema".into(),
            provider_ref: "backup-set:backup-1:shared-control".into(),
            captured_at: now,
            checksum: Some("checksum".into()),
            validation_status: "invalid".into(),
            validation_detail: detail.map(str::to_owned),
            retention_until: now,
            expires_at: Some(now),
            created_by: None,
            now,
        }
    }

    #[test]
    fn backup_insert_model_preserves_optional_validation_detail() {
        for detail in [Some("校验和错误"), None] {
            let model = backup_active_model(command(detail));
            assert_eq!(model.validation_detail, Set(detail.map(str::to_owned)));
        }
    }
}
