use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 可恢复的异步用户导入任务。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_user_import_job")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub requester_user_id: i64,
    pub background_job_id: i64,
    pub idempotency_key_hash: String,
    pub source_file_id: i64,
    pub source_name_snapshot: String,
    pub source_sha256: String,
    pub duplicate_policy: String,
    pub status: String,
    pub total_rows: i32,
    pub processed_rows: i32,
    pub success_count: i32,
    pub skipped_count: i32,
    pub failure_count: i32,
    pub cancel_requested: bool,
    pub error_report_file_id: Option<i64>,
    pub last_error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const DUPLICATE_SKIP_EXISTING: &str = "skip_existing";
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_SUCCEEDED: &str = "succeeded";
    pub const STATUS_PARTIAL: &str = "partial";
    pub const STATUS_FAILED: &str = "failed";
    pub const STATUS_CANCELLED: &str = "cancelled";

    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            Self::STATUS_PENDING | Self::STATUS_RUNNING
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            Self::STATUS_SUCCEEDED
                | Self::STATUS_PARTIAL
                | Self::STATUS_FAILED
                | Self::STATUS_CANCELLED
        )
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_import_row_result::Entity")]
    RowResults,
}

impl Related<super::user_import_row_result::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RowResults.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
