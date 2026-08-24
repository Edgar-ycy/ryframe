use chrono::{DateTime, Utc};

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
    pub retention_until: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<i64>,
    pub now: DateTime<Utc>,
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
