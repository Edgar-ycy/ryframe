use ryframe_application::ports::utc_timestamp;
use ryframe_kernel::{AppError, AppResult};
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};
use utoipa::ToSchema;

use ryframe_application::system::operations::ExportJobVo as ServiceExportJobVo;
use ryframe_application::{
    BackgroundJobQueueStats as ServiceBackgroundJobQueueStats,
    BackgroundJobVo as ServiceBackgroundJobVo,
};

/// 后台任务的公开视图，不包含内部载荷。
#[derive(Debug, Serialize, ToSchema)]
pub struct BackgroundJobVo {
    pub id: String,
    pub schedule_id: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub scheduled_for: Option<OffsetDateTime>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub available_at: OffsetDateTime,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub lease_until: Option<OffsetDateTime>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub completed_at: Option<OffsetDateTime>,
}

impl TryFrom<ServiceBackgroundJobVo> for BackgroundJobVo {
    type Error = AppError;

    fn try_from(value: ServiceBackgroundJobVo) -> AppResult<Self> {
        let ServiceBackgroundJobVo {
            id,
            schedule_id,
            scheduled_for,
            max_runtime_seconds,
            job_type,
            status,
            priority,
            available_at,
            attempts,
            max_attempts,
            lease_owner,
            lease_until,
            dedupe_key,
            last_error,
            created_at,
            updated_at,
            completed_at,
        } = value;
        let to_utc = |timestamp: OffsetDateTime, field| {
            timestamp.checked_to_offset(UtcOffset::UTC).ok_or_else(|| {
                AppError::Internal(format!("后台任务响应的 {field} 无法精确表示为 UTC"))
            })
        };
        Ok(Self {
            id,
            schedule_id,
            scheduled_for: scheduled_for
                .map(|timestamp| to_utc(timestamp, "scheduled_for"))
                .transpose()?,
            max_runtime_seconds,
            job_type,
            status,
            priority,
            available_at: to_utc(available_at, "available_at")?,
            attempts,
            max_attempts,
            lease_owner,
            lease_until: lease_until
                .map(|timestamp| to_utc(timestamp, "lease_until"))
                .transpose()?,
            dedupe_key,
            last_error,
            created_at: to_utc(created_at, "created_at")?,
            updated_at: to_utc(updated_at, "updated_at")?,
            completed_at: completed_at
                .map(|timestamp| to_utc(timestamp, "completed_at"))
                .transpose()?,
        })
    }
}

/// 后台任务队列统计。
#[derive(Debug, Serialize, ToSchema)]
pub struct BackgroundJobQueueStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

impl From<ServiceBackgroundJobQueueStats> for BackgroundJobQueueStats {
    fn from(value: ServiceBackgroundJobQueueStats) -> Self {
        let ServiceBackgroundJobQueueStats {
            total,
            pending,
            running,
            succeeded,
            dead,
            ready,
        } = value;
        Self {
            total,
            pending,
            running,
            succeeded,
            dead,
            ready,
        }
    }
}

/// 导出任务响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct ExportJobVo {
    pub id: String,
    pub resource: String,
    pub status: String,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<OffsetDateTime>,
    pub error_message: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub snapshot_at: OffsetDateTime,
    pub matched_rows: i64,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub notification_read_at: Option<OffsetDateTime>,
}

impl TryFrom<ServiceExportJobVo> for ExportJobVo {
    type Error = AppError;

    fn try_from(value: ServiceExportJobVo) -> AppResult<Self> {
        let ServiceExportJobVo {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at,
            error_message,
            snapshot_at,
            matched_rows,
            created_at,
            updated_at,
            completed_at,
            notification_read_at,
        } = value;
        let to_utc = |timestamp, field| {
            utc_timestamp::from_chrono(timestamp).map_err(|_| {
                AppError::Internal(format!("导出任务响应的 {field} 无法精确表示为 UTC"))
            })
        };
        Ok(Self {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at: expires_at
                .map(|timestamp| to_utc(timestamp, "expires_at"))
                .transpose()?,
            error_message,
            snapshot_at: to_utc(snapshot_at, "snapshot_at")?,
            matched_rows,
            created_at: to_utc(created_at, "created_at")?,
            updated_at: to_utc(updated_at, "updated_at")?,
            completed_at: completed_at
                .map(|timestamp| to_utc(timestamp, "completed_at"))
                .transpose()?,
            notification_read_at: notification_read_at
                .map(|timestamp| to_utc(timestamp, "notification_read_at"))
                .transpose()?,
        })
    }
}
