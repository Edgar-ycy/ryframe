use ryframe_application::ports::utc_timestamp;
use ryframe_application::system::platform as service;
use ryframe_kernel::{AppError, ValidatedPageQuery};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset};
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
    pub fn validate_page(&self) -> Result<ValidatedPageQuery, AppError> {
        ValidatedPageQuery::from_optional(
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_verified_at: Option<time::OffsetDateTime>,
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
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub captured_at: time::OffsetDateTime,
    pub checksum: Option<String>,
    pub validation_status: BackupPointValidationStatus,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub retention_until: time::OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<time::OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_restore_drill_at: Option<time::OffsetDateTime>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DataPlacementView {
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: String,
    pub state: TenantBusinessDataState,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub prechecked_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub queued_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub quiesced_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub frozen_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub copy_started_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub copy_completed_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub verified_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub cut_over_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub activated_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub succeeded_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub retention_until: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub finalized_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub failed_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub cancelled_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
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
            updated_at: response_timestamp(value.updated_at, "updated_at")?,
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
            prechecked_at: optional_response_timestamp(value.prechecked_at, "prechecked_at")?,
            queued_at: optional_response_timestamp(value.queued_at, "queued_at")?,
            quiesced_at: optional_response_timestamp(value.quiesced_at, "quiesced_at")?,
            frozen_at: optional_response_timestamp(value.frozen_at, "frozen_at")?,
            copy_started_at: optional_response_timestamp(value.copy_started_at, "copy_started_at")?,
            copy_completed_at: optional_response_timestamp(
                value.copy_completed_at,
                "copy_completed_at",
            )?,
            verified_at: optional_response_timestamp(value.verified_at, "verified_at")?,
            cut_over_at: optional_response_timestamp(value.cut_over_at, "cut_over_at")?,
            activated_at: optional_response_timestamp(value.activated_at, "activated_at")?,
            succeeded_at: optional_response_timestamp(value.succeeded_at, "succeeded_at")?,
            retention_until: optional_response_timestamp(value.retention_until, "retention_until")?,
            finalized_at: optional_response_timestamp(value.finalized_at, "finalized_at")?,
            failed_at: optional_response_timestamp(value.failed_at, "failed_at")?,
            cancelled_at: optional_response_timestamp(value.cancelled_at, "cancelled_at")?,
            created_at: response_timestamp(value.created_at, "created_at")?,
            updated_at: response_timestamp(value.updated_at, "updated_at")?,
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

fn response_timestamp(value: OffsetDateTime, field: &str) -> Result<OffsetDateTime, AppError> {
    value
        .checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| AppError::Internal(format!("租户数据响应的 {field} 无法精确表示为 UTC")))
}

fn optional_response_timestamp(
    value: Option<OffsetDateTime>,
    field: &str,
) -> Result<Option<OffsetDateTime>, AppError> {
    value
        .map(|instant| response_timestamp(instant, field))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use utoipa::OpenApi;

    #[test]
    fn tenant_data_responses_preserve_utc_precision_and_snapshot_schemas() {
        let mut placement = service::DataPlacementView {
            tenant_id: "tenant-a".into(),
            current_target_key: "target".into(),
            placement_generation: "2".into(),
            state: "active".into(),
            updated_at: fixture_timestamp(19, 123_456_700)
                .to_offset(UtcOffset::from_hms(5, 30, 0).unwrap()),
        };
        let response =
            serde_json::to_value(DataPlacementView::try_from(placement.clone()).unwrap()).unwrap();
        assert_eq!(response, serde_json::to_value(&placement).unwrap());
        assert_eq!(response["updated_at"], "1970-01-01T00:00:19.123456700Z");

        let migration = migration_fixture(true);
        let response =
            serde_json::to_value(MigrationView::try_from(migration.clone()).unwrap()).unwrap();
        assert_eq!(response, serde_json::to_value(&migration).unwrap());
        assert_eq!(response["created_at"], "1969-12-31T23:59:59Z");
        assert_eq!(response["queued_at"], "1970-01-01T00:00:02.120Z");
        assert_eq!(response["copy_completed_at"], "1970-01-01T00:00:06.123400Z");
        assert_eq!(response["updated_at"], "1970-01-01T00:00:15.123456700Z");

        let mut missing = migration_fixture(false);
        let response =
            serde_json::to_value(MigrationView::try_from(missing.clone()).unwrap()).unwrap();
        assert_eq!(response, serde_json::to_value(&missing).unwrap());
        for field in [
            "prechecked_at",
            "queued_at",
            "quiesced_at",
            "frozen_at",
            "copy_started_at",
            "copy_completed_at",
            "verified_at",
            "cut_over_at",
            "activated_at",
            "succeeded_at",
            "retention_until",
            "finalized_at",
            "failed_at",
            "cancelled_at",
        ] {
            assert_eq!(response.get(field), Some(&Value::Null), "{field}");
        }

        let out_of_range = time::Date::MIN
            .midnight()
            .assume_offset(UtcOffset::from_hms(1, 0, 0).unwrap());
        placement.updated_at = out_of_range;
        assert_timestamp_error(DataPlacementView::try_from(placement), "updated_at");
        let mut invalid_required = missing.clone();
        invalid_required.created_at = out_of_range;
        assert_timestamp_error(MigrationView::try_from(invalid_required), "created_at");
        missing.retention_until = Some(out_of_range);
        assert_timestamp_error(MigrationView::try_from(missing), "retention_until");

        let snapshot: Value =
            serde_json::from_str(include_str!("../../../../openapi/openapi.json")).unwrap();
        let generated = serde_json::to_value(crate::openapi::ApiDoc::openapi()).unwrap();
        for schema in ["DataPlacementView", "MigrationView"] {
            let path = format!("/components/schemas/{schema}");
            assert_eq!(
                generated.pointer(&path),
                Some(snapshot.pointer(&path).expect("快照包含租户数据响应契约"))
            );
        }
    }

    fn assert_timestamp_error<T: std::fmt::Debug>(result: Result<T, AppError>, field: &str) {
        let expected = format!("租户数据响应的 {field} 无法精确表示为 UTC");
        assert!(matches!(result.unwrap_err(), AppError::Internal(message) if message == expected));
    }

    fn fixture_timestamp(seconds: i64, nanos: u32) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
    }

    fn migration_fixture(with_phase_times: bool) -> service::MigrationView {
        let phase = |seconds, nanos| with_phase_times.then(|| fixture_timestamp(seconds, nanos));
        service::MigrationView {
            id: "7".into(),
            tenant_id: "tenant-a".into(),
            source_target_key: "source".into(),
            target_target_key: "target".into(),
            source_generation: "1".into(),
            target_generation: "2".into(),
            source_schema_fingerprint: "source-fingerprint".into(),
            target_schema_fingerprint: "target-fingerprint".into(),
            plan_hash: "plan-hash".into(),
            state: "succeeded".into(),
            operator_id: "42".into(),
            retention_hours: 168,
            error_code: None,
            prechecked_at: phase(1, 0),
            queued_at: phase(2, 120_000_000),
            quiesced_at: phase(3, 123_400_000),
            frozen_at: phase(4, 123_456_700),
            copy_started_at: phase(5, 0),
            copy_completed_at: phase(6, 123_400_000),
            verified_at: phase(7, 0),
            cut_over_at: phase(8, 0),
            activated_at: phase(9, 0),
            succeeded_at: phase(10, 0),
            retention_until: phase(11, 0),
            finalized_at: phase(12, 0),
            failed_at: phase(13, 0),
            cancelled_at: phase(14, 0),
            created_at: fixture_timestamp(-1, 0),
            updated_at: fixture_timestamp(15, 123_456_700),
            can_cancel: false,
            can_finalize: false,
            cancel_requested: false,
            finalize_requested: false,
            action_reasons: Vec::new(),
            items: Vec::new(),
        }
    }
}
