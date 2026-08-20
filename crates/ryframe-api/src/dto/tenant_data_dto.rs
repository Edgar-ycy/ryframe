use chrono::{DateTime, Utc};
use ryframe_application::system as service;
use ryframe_kernel::AppError;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::fixed_value::{
    BackupPointScope, BackupPointValidationStatus, DataTargetEligibility, DataTargetHealth,
    DataTargetKind, DataTargetMode, TenantBusinessDataState, TenantDataMigrationCleanupState,
    TenantDataMigrationItemState, TenantDataMigrationState,
};

const DATA_TARGET_DEFAULT_PAGE_SIZE: u64 = 20;
const DATA_TARGET_MAX_PAGE_SIZE: u64 = 100;

#[derive(Clone, Debug, Default, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct DataTargetListQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<u64>,
    /// 省略时只返回缓存健康快照；向导可指定 new_tenant 或 migration 做资格检查。
    pub eligible_for: Option<DataTargetEligibility>,
    pub tenant_id: Option<String>,
    #[param(max_length = 100)]
    pub q: Option<String>,
}

impl DataTargetListQuery {
    pub fn validate_page(
        &self,
    ) -> Result<ryframe_kernel::ValidatedPageQuery, ryframe_kernel::AppError> {
        ryframe_kernel::ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            ryframe_kernel::PaginationPolicy::new(
                DATA_TARGET_DEFAULT_PAGE_SIZE,
                DATA_TARGET_MAX_PAGE_SIZE,
            ),
        )
    }

    pub const fn max_page_size() -> u64 {
        DATA_TARGET_MAX_PAGE_SIZE
    }
}

#[derive(Clone, Debug, Default, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct BackupPointListQuery {
    /// 指定后仅返回该租户 tenant-scope 与目标 shard-scope 恢复点。
    pub tenant_id: Option<String>,
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct MigrationListQuery {
    #[param(minimum = 1, maximum = 100)]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MigrationPreviewDto {
    #[validate(length(min = 2, max = 64))]
    pub target_key: String,
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,19}$")]
    pub expected_placement_generation: String,
}

#[derive(Clone, Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMigrationDto {
    #[validate(length(min = 2, max = 64))]
    pub target_key: String,
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,19}$")]
    pub expected_placement_generation: String,
    #[validate(length(equal = 64))]
    #[schema(pattern = r"^[a-f0-9]{64}$")]
    pub plan_hash: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataTargetSummary {
    pub key: String,
    pub display_name: Option<String>,
    pub mode: DataTargetMode,
    pub kind: DataTargetKind,
    pub region: Option<String>,
    pub health: DataTargetHealth,
    pub schema_fingerprint: Option<String>,
    pub connected: bool,
    pub pool_max_connections: Option<u32>,
    pub active_leases: usize,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataTargetDetail {
    #[serde(flatten)]
    pub target: DataTargetSummary,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub reserved_connections: u32,
    pub max_total_connections: u32,
    pub open_targets: usize,
    pub opening_targets: usize,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackupPointView {
    pub id: String,
    pub scope: BackupPointScope,
    pub tenant_id: Option<String>,
    pub target_key: String,
    pub placement_generation: Option<String>,
    pub schema_fingerprint: String,
    pub captured_at: DateTime<Utc>,
    pub checksum: Option<String>,
    pub validation_status: BackupPointValidationStatus,
    pub retention_until: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_restore_drill_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataPlacementView {
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: String,
    pub state: TenantBusinessDataState,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MigrationImpact {
    pub stop_write: bool,
    pub catalog_table_count: usize,
    pub retention_hours: i32,
    pub rollback_boundary: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
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

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MigrationItemView {
    pub id: String,
    pub table_name: String,
    pub copy_order: i32,
    pub state: TenantDataMigrationItemState,
    pub cursor: Option<serde_json::Value>,
    pub source_row_count: Option<String>,
    pub target_row_count: Option<String>,
    pub source_digest: Option<String>,
    pub target_digest: Option<String>,
    pub error_code: Option<String>,
    pub cleanup_state: TenantDataMigrationCleanupState,
    pub cleanup_row_count: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
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
    pub state: TenantDataMigrationState,
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

impl TryFrom<service::DataTargetSummary> for DataTargetSummary {
    type Error = AppError;

    fn try_from(value: service::DataTargetSummary) -> Result<Self, Self::Error> {
        let mode = DataTargetMode::try_from(value.mode.as_str())?;
        let kind = DataTargetKind::try_from(value.kind.as_str())?;
        let health = DataTargetHealth::try_from(value.health.as_str())?;
        Ok(Self {
            key: value.key,
            display_name: value.display_name,
            mode,
            kind,
            region: value.region,
            health,
            schema_fingerprint: value.schema_fingerprint,
            connected: value.connected,
            pool_max_connections: value.pool_max_connections,
            active_leases: value.active_leases,
            eligible: value.eligible,
            reasons: value.reasons,
        })
    }
}

impl TryFrom<service::DataTargetDetail> for DataTargetDetail {
    type Error = AppError;

    fn try_from(value: service::DataTargetDetail) -> Result<Self, Self::Error> {
        Ok(Self {
            target: value.target.try_into()?,
            last_verified_at: value.last_verified_at,
            reserved_connections: value.reserved_connections,
            max_total_connections: value.max_total_connections,
            open_targets: value.open_targets,
            opening_targets: value.opening_targets,
        })
    }
}

impl TryFrom<service::BackupPointView> for BackupPointView {
    type Error = AppError;

    fn try_from(value: service::BackupPointView) -> Result<Self, Self::Error> {
        let scope = BackupPointScope::try_from(value.scope.as_str())?;
        let validation_status =
            BackupPointValidationStatus::try_from(value.validation_status.as_str())?;
        Ok(Self {
            id: value.id,
            scope,
            tenant_id: value.tenant_id,
            target_key: value.target_key,
            placement_generation: value.placement_generation,
            schema_fingerprint: value.schema_fingerprint,
            captured_at: value.captured_at,
            checksum: value.checksum,
            validation_status,
            retention_until: value.retention_until,
            expires_at: value.expires_at,
            last_restore_drill_at: value.last_restore_drill_at,
        })
    }
}

impl TryFrom<service::DataPlacementView> for DataPlacementView {
    type Error = AppError;

    fn try_from(value: service::DataPlacementView) -> Result<Self, Self::Error> {
        let state = TenantBusinessDataState::try_from(value.state.as_str())?;
        Ok(Self {
            tenant_id: value.tenant_id,
            current_target_key: value.current_target_key,
            placement_generation: value.placement_generation,
            state,
            updated_at: value.updated_at,
        })
    }
}

impl From<service::MigrationPreview> for MigrationPreview {
    fn from(value: service::MigrationPreview) -> Self {
        Self {
            tenant_id: value.tenant_id,
            source_target_key: value.source_target_key,
            target_target_key: value.target_target_key,
            expected_placement_generation: value.expected_placement_generation,
            target_generation: value.target_generation,
            plan_hash: value.plan_hash,
            eligible: value.eligible,
            blockers: value.blockers,
            warnings: value.warnings,
            impact: MigrationImpact {
                stop_write: value.impact.stop_write,
                catalog_table_count: value.impact.catalog_table_count,
                retention_hours: value.impact.retention_hours,
                rollback_boundary: value.impact.rollback_boundary,
            },
        }
    }
}

impl TryFrom<service::MigrationView> for MigrationView {
    type Error = AppError;

    fn try_from(value: service::MigrationView) -> Result<Self, Self::Error> {
        let state = TenantDataMigrationState::try_from(value.state.as_str())?;
        let items = value
            .items
            .into_iter()
            .map(MigrationItemView::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id: value.id,
            tenant_id: value.tenant_id,
            source_target_key: value.source_target_key,
            target_target_key: value.target_target_key,
            source_generation: value.source_generation,
            target_generation: value.target_generation,
            source_schema_fingerprint: value.source_schema_fingerprint,
            target_schema_fingerprint: value.target_schema_fingerprint,
            plan_hash: value.plan_hash,
            state,
            operator_id: value.operator_id,
            retention_hours: value.retention_hours,
            error_code: value.error_code,
            prechecked_at: value.prechecked_at,
            queued_at: value.queued_at,
            quiesced_at: value.quiesced_at,
            frozen_at: value.frozen_at,
            copy_started_at: value.copy_started_at,
            copy_completed_at: value.copy_completed_at,
            verified_at: value.verified_at,
            cut_over_at: value.cut_over_at,
            activated_at: value.activated_at,
            succeeded_at: value.succeeded_at,
            retention_until: value.retention_until,
            finalized_at: value.finalized_at,
            failed_at: value.failed_at,
            cancelled_at: value.cancelled_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            can_cancel: value.can_cancel,
            can_finalize: value.can_finalize,
            cancel_requested: value.cancel_requested,
            finalize_requested: value.finalize_requested,
            action_reasons: value.action_reasons,
            items,
        })
    }
}

impl TryFrom<service::MigrationItemView> for MigrationItemView {
    type Error = AppError;

    fn try_from(value: service::MigrationItemView) -> Result<Self, Self::Error> {
        let state = TenantDataMigrationItemState::try_from(value.state.as_str())?;
        let cleanup_state =
            TenantDataMigrationCleanupState::try_from(value.cleanup_state.as_str())?;
        Ok(Self {
            id: value.id,
            table_name: value.table_name,
            copy_order: value.copy_order,
            state,
            cursor: value.cursor,
            source_row_count: value.source_row_count,
            target_row_count: value.target_row_count,
            source_digest: value.source_digest,
            target_digest: value.target_digest,
            error_code: value.error_code,
            cleanup_state,
            cleanup_row_count: value.cleanup_row_count,
        })
    }
}
