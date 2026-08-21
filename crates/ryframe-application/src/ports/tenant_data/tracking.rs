use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;

use crate::{PersistenceFuture, ports::jobs::BackgroundJobTransaction};

pub const MIGRATION_STATE_PRECHECKING: &str = "prechecking";
pub const MIGRATION_STATE_QUEUED: &str = "queued";
pub const MIGRATION_STATE_QUIESCING: &str = "quiescing";
pub const MIGRATION_STATE_FROZEN: &str = "frozen";
pub const MIGRATION_STATE_COPYING: &str = "copying";
pub const MIGRATION_STATE_VERIFYING: &str = "verifying";
pub const MIGRATION_STATE_CUTTING_OVER: &str = "cutting_over";
pub const MIGRATION_STATE_ACTIVATING: &str = "activating";
pub const MIGRATION_STATE_SUCCEEDED: &str = "succeeded";
pub const MIGRATION_STATE_RETENTION_PENDING: &str = "retention_pending";
pub const MIGRATION_STATE_FINALIZED: &str = "finalized";
pub const MIGRATION_STATE_FAILED: &str = "failed";
pub const MIGRATION_STATE_CANCELLED: &str = "cancelled";

pub const MIGRATION_ITEM_STATE_PENDING: &str = "pending";
pub const MIGRATION_ITEM_STATE_COPYING: &str = "copying";
pub const MIGRATION_ITEM_STATE_COPIED: &str = "copied";
pub const MIGRATION_ITEM_STATE_VERIFYING: &str = "verifying";
pub const MIGRATION_ITEM_STATE_VERIFIED: &str = "verified";
pub const MIGRATION_ITEM_CLEANUP_PENDING: &str = "pending";
pub const MIGRATION_ITEM_CLEANUP_CLEANING: &str = "cleaning";
pub const MIGRATION_ITEM_CLEANUP_CLEANED: &str = "cleaned";

pub const PLACEMENT_STATE_ACTIVE: &str = "active";
pub const PLACEMENT_STATE_MAINTENANCE: &str = "maintenance";

#[derive(Clone, Debug, PartialEq)]
pub struct TenantDataMigrationRecord {
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
    pub cancel_idempotency_key_hash: Option<String>,
    pub finalize_idempotency_key_hash: Option<String>,
    pub state: String,
    pub switch_token: String,
    pub operator_id: i64,
    pub cancelled_by: Option<i64>,
    pub finalized_by: Option<i64>,
    pub background_job_id: Option<i64>,
    pub retention_hours: i32,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
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
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub finalize_requested_at: Option<DateTime<Utc>>,
    pub cleanup_ready_at: Option<DateTime<Utc>>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantDataMigrationRecord {
    pub const STATE_PRECHECKING: &'static str = MIGRATION_STATE_PRECHECKING;
    pub const STATE_QUEUED: &'static str = MIGRATION_STATE_QUEUED;
    pub const STATE_QUIESCING: &'static str = MIGRATION_STATE_QUIESCING;
    pub const STATE_FROZEN: &'static str = MIGRATION_STATE_FROZEN;
    pub const STATE_COPYING: &'static str = MIGRATION_STATE_COPYING;
    pub const STATE_VERIFYING: &'static str = MIGRATION_STATE_VERIFYING;
    pub const STATE_CUTTING_OVER: &'static str = MIGRATION_STATE_CUTTING_OVER;
    pub const STATE_ACTIVATING: &'static str = MIGRATION_STATE_ACTIVATING;
    pub const STATE_SUCCEEDED: &'static str = MIGRATION_STATE_SUCCEEDED;
    pub const STATE_RETENTION_PENDING: &'static str = MIGRATION_STATE_RETENTION_PENDING;
    pub const STATE_FINALIZED: &'static str = MIGRATION_STATE_FINALIZED;
    pub const STATE_FAILED: &'static str = MIGRATION_STATE_FAILED;
    pub const STATE_CANCELLED: &'static str = MIGRATION_STATE_CANCELLED;

    pub fn can_cancel(&self) -> bool {
        matches!(
            self.state.as_str(),
            MIGRATION_STATE_PRECHECKING
                | MIGRATION_STATE_QUEUED
                | MIGRATION_STATE_QUIESCING
                | MIGRATION_STATE_FROZEN
                | MIGRATION_STATE_COPYING
                | MIGRATION_STATE_VERIFYING
        )
    }

    pub fn can_finalize(&self) -> bool {
        self.state == MIGRATION_STATE_RETENTION_PENDING
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantDataMigrationItemRecord {
    pub id: i64,
    pub migration_id: i64,
    pub table_name: String,
    pub copy_order: i32,
    pub state: String,
    pub cursor_json: Option<JsonValue>,
    pub source_row_count: Option<i64>,
    pub target_row_count: Option<i64>,
    pub source_digest: Option<String>,
    pub target_digest: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub copy_started_at: Option<DateTime<Utc>>,
    pub copied_at: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
    pub cleanup_state: String,
    pub cleanup_row_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantDataMigrationItemRecord {
    pub const STATE_PENDING: &'static str = MIGRATION_ITEM_STATE_PENDING;
    pub const STATE_COPYING: &'static str = MIGRATION_ITEM_STATE_COPYING;
    pub const STATE_COPIED: &'static str = MIGRATION_ITEM_STATE_COPIED;
    pub const STATE_VERIFYING: &'static str = MIGRATION_ITEM_STATE_VERIFYING;
    pub const STATE_VERIFIED: &'static str = MIGRATION_ITEM_STATE_VERIFIED;
    pub const CLEANUP_PENDING: &'static str = MIGRATION_ITEM_CLEANUP_PENDING;
    pub const CLEANUP_CLEANING: &'static str = MIGRATION_ITEM_CLEANUP_CLEANING;
    pub const CLEANUP_CLEANED: &'static str = MIGRATION_ITEM_CLEANUP_CLEANED;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantDataPlacementRecord {
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: i64,
    pub state: String,
    pub switch_token: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantDataPlacementRecord {
    pub const STATE_ACTIVE: &'static str = PLACEMENT_STATE_ACTIVE;
    pub const STATE_MAINTENANCE: &'static str = PLACEMENT_STATE_MAINTENANCE;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantDataBackupPointRecord {
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
    pub last_restore_drill_at: Option<DateTime<Utc>>,
    pub created_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct CreateTenantDataMigrationRecord {
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
pub struct TenantOperationLeaseRecord {
    pub tenant_id: String,
    pub owner_token: String,
    pub operation: String,
    pub resource_type: String,
    pub resource_id: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantMigrationContextRecord {
    pub authorization_epoch: i32,
}

/// 租户数据迁移状态机所拥有的控制库工作单元。
pub trait TenantDataMigrationTransaction: Send + Sync {
    fn background_jobs(&self) -> &dyn BackgroundJobTransaction;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn acquire_lease(&self, lease: TenantOperationLeaseRecord) -> PersistenceFuture<'_, ()>;

    fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> PersistenceFuture<'a, bool>;

    fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> PersistenceFuture<'a, TenantMigrationContextRecord>;

    fn increment_runtime_epoch<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, i64>;

    fn lock_placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, TenantDataPlacementRecord>;

    fn save_placement(
        &self,
        placement: TenantDataPlacementRecord,
    ) -> PersistenceFuture<'_, TenantDataPlacementRecord>;

    fn lock_migration(&self, id: i64) -> PersistenceFuture<'_, TenantDataMigrationRecord>;

    fn lock_active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>>;

    fn insert_migration(
        &self,
        command: CreateTenantDataMigrationRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationRecord>;

    fn save_migration(
        &self,
        migration: TenantDataMigrationRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationRecord>;

    fn lock_item(&self, id: i64) -> PersistenceFuture<'_, TenantDataMigrationItemRecord>;

    fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord>;

    fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

/// 租户数据迁移状态机所需的控制库持久化端口。
pub trait TenantDataMigrationPersistencePort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn occupied_target_keys<'a>(
        &'a self,
        configured_target_keys: &'a [String],
    ) -> PersistenceFuture<'a, HashSet<String>>;

    fn placement<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataPlacementRecord>>;

    fn migration(&self, id: i64) -> PersistenceFuture<'_, Option<TenantDataMigrationRecord>>;

    fn migrations_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<TenantDataMigrationRecord>>;

    fn recoverable_migrations(
        &self,
        after_id: Option<i64>,
        limit: u64,
    ) -> PersistenceFuture<'_, Vec<TenantDataMigrationRecord>>;

    fn migration_by_create_key<'a>(
        &'a self,
        key_hash: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>>;

    fn active_migration_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantDataMigrationRecord>>;

    fn items(&self, migration_id: i64)
    -> PersistenceFuture<'_, Vec<TenantDataMigrationItemRecord>>;

    fn insert_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord>;

    fn save_item(
        &self,
        item: TenantDataMigrationItemRecord,
    ) -> PersistenceFuture<'_, TenantDataMigrationItemRecord>;

    fn backup_points_for_target<'a>(
        &'a self,
        target_key: &'a str,
        tenant_id: Option<&'a str>,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<TenantDataBackupPointRecord>>;

    fn has_validated_backup<'a>(
        &'a self,
        migration: &'a TenantDataMigrationRecord,
        not_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn TenantDataMigrationTransaction>>;
}
