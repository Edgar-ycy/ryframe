use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 单次领取的时间事实；租约失效的关闭时间不代表 Worker 实际完成时间。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_background_job_attempt")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub job_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub sequence: i64,
    pub available_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub outcome: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
