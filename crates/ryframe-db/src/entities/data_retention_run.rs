use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 一次数据保留任务的汇总记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_data_retention_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub background_job_id: i64,
    pub trigger_kind: String,
    pub status: String,
    pub policy_snapshot: Json,
    pub eligible_counts: Json,
    pub deleted_counts: Json,
    pub remaining_counts: Json,
    pub requested_by: Option<i64>,
    pub error_summary: Option<String>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub started_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

impl Model {
    pub const TRIGGER_SCHEDULED: &str = "scheduled";
    pub const TRIGGER_MANUAL: &str = "manual";
    pub const STATUS_PENDING: &str = "pending";
    pub const STATUS_RUNNING: &str = "running";
    pub const STATUS_SUCCEEDED: &str = "succeeded";
    pub const STATUS_PARTIAL: &str = "partial";
    pub const STATUS_FAILED: &str = "failed";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
