use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 单次领取的时间事实；租约失效的关闭时间不代表 Worker 实际完成时间。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_background_job_attempt")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub job_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub sequence: i64,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub available_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub started_at: OffsetDateTime,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(
        default,
        serialize_with = "ryframe_application::ports::utc_timestamp::serialize_optional",
        deserialize_with = "ryframe_application::ports::utc_timestamp::deserialize_optional"
    )]
    pub closed_at: Option<OffsetDateTime>,
    pub outcome: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
