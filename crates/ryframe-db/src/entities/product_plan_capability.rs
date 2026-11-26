use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_product_plan_capability")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub plan_version_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub capability_code: String,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: Json,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
