use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 调度计划每次触发或跳过的不可变历史记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_job_schedule_execution")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub schedule_id: i64,
    pub schedule_name_snapshot: String,
    pub handler_key_snapshot: String,
    pub fire_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<i64>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Model {
    pub const TRIGGER_SCHEDULED: &str = "scheduled";
    pub const TRIGGER_MISFIRE: &str = "misfire";
    pub const TRIGGER_MANUAL: &str = "manual";
    pub const OUTCOME_ENQUEUED: &str = "enqueued";
    pub const OUTCOME_SKIPPED_MISFIRE: &str = "skipped_misfire";
    pub const OUTCOME_SKIPPED_CONCURRENCY: &str = "skipped_concurrency";
    pub const OUTCOME_TARGET_UNAVAILABLE: &str = "target_unavailable";
    pub const OUTCOME_INVALID_CONFIGURATION: &str = "invalid_configuration";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::background_job::Entity",
        from = "Column::BackgroundJobId",
        to = "super::background_job::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    BackgroundJob,
}

impl Related<super::background_job::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BackgroundJob.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
