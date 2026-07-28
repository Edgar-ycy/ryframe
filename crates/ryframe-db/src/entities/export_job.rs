use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 面向用户的异步导出任务。
///
/// 该表与内部 `sys_background_job` 分离，避免消息、审计等内部任务的状态语义泄漏到公开 API。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_export_job")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub requester_id: i64,
    pub resource: String,
    pub background_job_id: i64,
    pub request_params: Json,
    pub permission_code: String,
    pub status: String,
    pub result_file_id: Option<i64>,
    pub result_file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Model {
    pub const STATUS_QUEUED: &str = "queued";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_SUCCEEDED: &str = "succeeded";
    pub const STATUS_FAILED: &str = "failed";
    pub const STATUS_CANCELLED: &str = "cancelled";
    pub const STATUS_EXPIRED: &str = "expired";

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            Self::STATUS_SUCCEEDED
                | Self::STATUS_FAILED
                | Self::STATUS_CANCELLED
                | Self::STATUS_EXPIRED
        )
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
