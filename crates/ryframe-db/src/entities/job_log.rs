use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_job_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub job_name: String,
    pub job_group: String,
    pub message: String,
    pub status: String,
    pub error_msg: Option<String>,
    pub cost_ms: i64,
    pub start_time: DateTime<Utc>,
}

impl Model {
    pub const STATUS_SUCCESS: &str = "1";
    pub const STATUS_FAIL: &str = "0";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
