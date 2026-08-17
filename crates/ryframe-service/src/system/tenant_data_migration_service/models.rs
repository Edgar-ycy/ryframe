use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct DataTargetSummary {
    pub key: String,
    pub display_name: Option<String>,
    pub mode: String,
    pub kind: String,
    pub region: Option<String>,
    pub health: String,
    pub schema_fingerprint: Option<String>,
    pub connected: bool,
    pub pool_max_connections: Option<u32>,
    pub active_leases: usize,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataTargetDetail {
    #[serde(flatten)]
    pub target: DataTargetSummary,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub reserved_connections: u32,
    pub max_total_connections: u32,
    pub open_targets: usize,
    pub opening_targets: usize,
}

#[derive(Clone, Debug, Default)]
pub struct DataTargetListParams {
    pub eligible_for: Option<String>,
    pub tenant_id: Option<String>,
    pub q: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct BackupPointListParams {
    pub tenant_id: Option<String>,
    pub limit: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupPointView {
    pub id: String,
    pub scope: String,
    pub tenant_id: Option<String>,
    pub target_key: String,
    pub placement_generation: Option<String>,
    pub schema_fingerprint: String,
    pub captured_at: DateTime<Utc>,
    pub checksum: Option<String>,
    pub validation_status: String,
    pub retention_until: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_restore_drill_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataPlacementView {
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: String,
    pub state: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct MigrationPreviewRequest {
    pub target_key: String,
    pub expected_placement_generation: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationImpact {
    pub stop_write: bool,
    pub catalog_table_count: usize,
    pub retention_hours: i32,
    pub rollback_boundary: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationPreview {
    pub tenant_id: String,
    pub source_target_key: String,
    pub target_target_key: String,
    pub expected_placement_generation: String,
    pub target_generation: String,
    pub plan_hash: String,
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub impact: MigrationImpact,
}

#[derive(Clone, Debug)]
pub struct CreateMigrationCommand {
    pub target_key: String,
    pub expected_placement_generation: i64,
    pub plan_hash: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug)]
pub struct MigrationActionCommand {
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationItemView {
    pub id: String,
    pub table_name: String,
    pub copy_order: i32,
    pub state: String,
    pub cursor: Option<serde_json::Value>,
    pub source_row_count: Option<String>,
    pub target_row_count: Option<String>,
    pub source_digest: Option<String>,
    pub target_digest: Option<String>,
    pub error_code: Option<String>,
    pub cleanup_state: String,
    pub cleanup_row_count: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationView {
    pub id: String,
    pub tenant_id: String,
    pub source_target_key: String,
    pub target_target_key: String,
    pub source_generation: String,
    pub target_generation: String,
    pub source_schema_fingerprint: String,
    pub target_schema_fingerprint: String,
    pub plan_hash: String,
    pub state: String,
    pub operator_id: String,
    pub retention_hours: i32,
    pub error_code: Option<String>,
    pub prechecked_at: Option<DateTime<Utc>>,
    pub queued_at: Option<DateTime<Utc>>,
    pub quiesced_at: Option<DateTime<Utc>>,
    pub frozen_at: Option<DateTime<Utc>>,
    pub copy_started_at: Option<DateTime<Utc>>,
    pub copy_completed_at: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
    pub cut_over_at: Option<DateTime<Utc>>,
    pub activated_at: Option<DateTime<Utc>>,
    pub succeeded_at: Option<DateTime<Utc>>,
    pub retention_until: Option<DateTime<Utc>>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub can_cancel: bool,
    pub can_finalize: bool,
    pub cancel_requested: bool,
    pub finalize_requested: bool,
    pub action_reasons: Vec<String>,
    pub items: Vec<MigrationItemView>,
}
