//! 租户数据放置实体。

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 控制库中租户业务数据的权威放置记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant_data_placement")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: String,
    pub current_target_key: String,
    pub placement_generation: i64,
    pub state: String,
    pub switch_token: String,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

impl Model {
    pub const STATE_PROVISIONING: &str = "provisioning";
    pub const STATE_ACTIVE: &str = "active";
    pub const STATE_MAINTENANCE: &str = "maintenance";
    pub const STATE_FAILED: &str = "failed";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
