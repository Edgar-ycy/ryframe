use ryframe_application::ports::utc_timestamp;
use ryframe_application::system::identity::{
    UserImportJobVo as ServiceJob, UserImportRowVo as ServiceRow,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct UserImportJobVo {
    pub id: String,
    pub source_name: String,
    pub requester_username: Option<String>,
    pub duplicate_policy: String,
    pub status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_count: i32,
    pub skipped_count: i32,
    pub failure_count: i32,
    pub cancel_requested: bool,
    pub report_available: bool,
    pub last_error: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub started_at: Option<time::OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub completed_at: Option<time::OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: time::OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: time::OffsetDateTime,
}

impl From<ServiceJob> for UserImportJobVo {
    fn from(value: ServiceJob) -> Self {
        Self {
            id: value.id,
            source_name: value.source_name,
            requester_username: value.requester_username,
            duplicate_policy: value.duplicate_policy,
            status: value.status,
            total_rows: value.total_rows,
            processed_rows: value.processed_rows,
            success_count: value.success_count,
            skipped_count: value.skipped_count,
            failure_count: value.failure_count,
            cancel_requested: value.cancel_requested,
            report_available: value.report_available,
            last_error: value.last_error,
            started_at: value.started_at,
            completed_at: value.completed_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserImportRowVo {
    pub row_number: i32,
    pub username: String,
    pub outcome: String,
    pub code: String,
    pub message: String,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: time::OffsetDateTime,
}

impl From<ServiceRow> for UserImportRowVo {
    fn from(value: ServiceRow) -> Self {
        Self {
            row_number: value.row_number,
            username: value.username,
            outcome: value.outcome,
            code: value.code,
            message: value.message,
            created_at: value.created_at,
        }
    }
}
