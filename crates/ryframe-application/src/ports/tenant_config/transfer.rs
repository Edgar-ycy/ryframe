use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};
use serde_json::Value as JsonValue;
use std::collections::BTreeSet;

use crate::{
    ports::authorization::AuthorizationMirrorTransaction,
    ports::jobs::BackgroundJobTransaction,
    ports::product::ProductTransactionPort,
    system::{TenantConfigPackageResources, TenantConfigTargetCatalog},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantConfigurationFenceRecord {
    pub configuration_version: i64,
    pub authorization_epoch: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantConfigBundleRecord {
    pub id: i64,
    pub tenant_id: String,
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name_snapshot: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub file_id: Option<i64>,
    pub sha256: Option<String>,
    pub resource_counts: JsonValue,
    pub item_count: i32,
    pub status: String,
    pub background_job_id: Option<i64>,
    pub idempotency_key_hash: Option<String>,
    pub created_by: i64,
    pub error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantConfigBundleRecord {
    pub const ORIGIN_GENERATED: &'static str = "generated";
    pub const ORIGIN_UPLOADED: &'static str = "uploaded";
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_RUNNING: &'static str = "running";
    pub const STATUS_SUCCEEDED: &'static str = "succeeded";
    pub const STATUS_FAILED: &'static str = "failed";
    pub const STATUS_EXPIRED: &'static str = "expired";
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantConfigTransferRecord {
    pub id: i64,
    pub tenant_id: String,
    pub bundle_id: i64,
    pub idempotency_key_hash: String,
    pub request_kind: String,
    pub request_fingerprint: String,
    pub status: String,
    pub target_configuration_version: i64,
    pub target_authorization_epoch: i32,
    pub plan_hash: Option<String>,
    pub preview_calculated_at: Option<DateTime<Utc>>,
    pub preview_background_job_id: Option<i64>,
    pub apply_background_job_id: Option<i64>,
    pub rollback_background_job_id: Option<i64>,
    pub snapshot_file_id: Option<i64>,
    pub applied_configuration_version: Option<i64>,
    pub applied_authorization_epoch: Option<i32>,
    pub change_counts: JsonValue,
    pub error_summary: Option<String>,
    pub requested_by: i64,
    pub rollback_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantConfigTransferRecord {
    pub const STATUS_PREVIEW_READY: &'static str = "preview_ready";
    pub const STATUS_PREVIEW_PENDING: &'static str = "preview_pending";
    pub const STATUS_PREVIEWING: &'static str = "previewing";
    pub const STATUS_PREVIEWED: &'static str = "previewed";
    pub const STATUS_APPLY_PENDING: &'static str = "apply_pending";
    pub const STATUS_APPLYING: &'static str = "applying";
    pub const STATUS_APPLIED: &'static str = "applied";
    pub const STATUS_ROLLBACK_PENDING: &'static str = "rollback_pending";
    pub const STATUS_ROLLING_BACK: &'static str = "rolling_back";
    pub const STATUS_ROLLED_BACK: &'static str = "rolled_back";
    pub const STATUS_FAILED: &'static str = "failed";
}

#[derive(Clone, Debug, PartialEq)]
pub struct TenantConfigTransferItemRecord {
    pub id: i64,
    pub tenant_id: String,
    pub transfer_id: i64,
    pub resource_type: String,
    pub stable_key: String,
    pub display_name: String,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantConfigTransferItemRecord {
    pub const ACTION_CREATE: &'static str = "create";
    pub const ACTION_UPDATE: &'static str = "update";
    pub const ACTION_UNCHANGED: &'static str = "unchanged";
    pub const ACTION_CONFLICT: &'static str = "conflict";
    pub const ACTION_BLOCKED: &'static str = "blocked";
    pub const OUTCOME_PENDING: &'static str = "pending";
    pub const OUTCOME_APPLIED: &'static str = "applied";
    pub const OUTCOME_SKIPPED: &'static str = "skipped";
    pub const OUTCOME_FAILED: &'static str = "failed";
    pub const OUTCOME_ROLLED_BACK: &'static str = "rolled_back";
}

#[derive(Clone, Debug)]
pub struct TenantConfigOperationLeaseRecord {
    pub tenant_id: String,
    pub owner_token: String,
    pub operation: String,
    pub resource_type: String,
    pub resource_id: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantConfigRequesterRecord {
    pub tenant_id: String,
    pub user_id: i64,
    pub tenant_authorization_epoch: i32,
    pub user_authorization_version: i32,
}

/// 租户配置迁移用例所拥有的控制库工作单元。
#[async_trait::async_trait]
pub trait TenantConfigTransferTransaction: Send + Sync {
    fn background_jobs(&self) -> &dyn BackgroundJobTransaction;

    fn product(&self) -> &dyn ProductTransactionPort;

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction;

    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn lock_tenant_configuration<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: Option<&'a str>,
    ) -> ryframe_kernel::AppResult<TenantConfigurationFenceRecord>;

    async fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i64>;

    async fn acquire_lease(
        &self,
        lease: TenantConfigOperationLeaseRecord,
    ) -> ryframe_kernel::AppResult<()>;

    async fn renew_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        expires_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn release_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn insert_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigBundleRecord>;

    async fn lock_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>>;

    async fn lock_bundle_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>>;

    async fn find_bundle_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        created_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>>;

    async fn update_bundle(
        &self,
        bundle: TenantConfigBundleRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigBundleRecord>;

    async fn insert_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigTransferRecord>;

    async fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>>;

    async fn lock_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>>;

    async fn update_transfer(
        &self,
        transfer: TenantConfigTransferRecord,
    ) -> ryframe_kernel::AppResult<TenantConfigTransferRecord>;

    async fn replace_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        items: Vec<TenantConfigTransferItemRecord>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn list_items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigTransferItemRecord>>;

    async fn tenant_name<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<String>;

    async fn ensure_config_package_file_ready<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantConfigPackageResources>;

    async fn apply_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a TenantConfigPackageResources,
        plan_items: &'a [TenantConfigTransferItemRecord],
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn ensure_rollback_references_safe<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<()>;

    async fn restore_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        snapshot: &'a TenantConfigPackageResources,
        transfer_id: i64,
        target_catalog: &'a TenantConfigTargetCatalog,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn ensure_requester_snapshot<'a>(
        &'a self,
        tenant_id: &'a str,
        requester: TenantConfigRequesterRecord,
        fence: TenantConfigurationFenceRecord,
        database_now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn ensure_role_quota<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_items: &'a [TenantConfigTransferItemRecord],
    ) -> ryframe_kernel::AppResult<()>;

    async fn mark_plan_outcome<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        outcome: &'a str,
    ) -> ryframe_kernel::AppResult<()>;

    async fn dead_background_job_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        candidates: &'a [i64],
    ) -> ryframe_kernel::AppResult<BTreeSet<i64>>;

    async fn commit_audited(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()>;

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()>;
}

/// 租户配置迁移用例所需的控制库持久化端口。
#[async_trait::async_trait]
pub trait TenantConfigTransferPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn bundle_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigBundleRecord>>;

    async fn transfer_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigTransferRecord>>;

    async fn item_page<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
        page: ValidatedPageQuery,
    ) -> ryframe_kernel::AppResult<PageResult<TenantConfigTransferItemRecord>>;

    async fn find_bundle<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigBundleRecord>>;

    async fn find_bundles<'a>(
        &'a self,
        tenant_id: &'a str,
        ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigBundleRecord>>;

    async fn find_transfer<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>>;

    async fn find_transfer_by_background_job(
        &self,
        background_job_id: i64,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>>;

    async fn find_transfer_by_idempotency_key<'a>(
        &'a self,
        tenant_id: &'a str,
        requested_by: i64,
        idempotency_key_hash: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantConfigTransferRecord>>;

    async fn items<'a>(
        &'a self,
        tenant_id: &'a str,
        transfer_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<TenantConfigTransferItemRecord>>;

    async fn cache_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i64>;

    async fn load_resources<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantConfigPackageResources>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn TenantConfigTransferTransaction>>;
}
