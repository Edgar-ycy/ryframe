use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use ryframe_application::system::ExportJobVo as ServiceExportJobVo;
use ryframe_application::{
    BackgroundJobQueueStats as ServiceBackgroundJobQueueStats,
    BackgroundJobVo as ServiceBackgroundJobVo,
};

/// 后台任务的公开视图，不包含内部载荷。
#[derive(Debug, Serialize, ToSchema)]
pub struct BackgroundJobVo {
    pub id: String,
    pub schedule_id: Option<String>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<ServiceBackgroundJobVo> for BackgroundJobVo {
    fn from(value: ServiceBackgroundJobVo) -> Self {
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
        Self {
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
        }
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
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub notification_read_at: Option<DateTime<Utc>>,
}

impl From<ServiceExportJobVo> for ExportJobVo {
    fn from(value: ServiceExportJobVo) -> Self {
        let ServiceExportJobVo {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at,
            error_message,
            created_at,
            updated_at,
            completed_at,
            notification_read_at,
        } = value;
        Self {
            id,
            resource,
            status,
            result_file_name,
            content_type,
            file_size,
            expires_at,
            error_message,
            created_at,
            updated_at,
            completed_at,
            notification_read_at,
        }
    }
}
