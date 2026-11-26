use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, PrimitiveDateTime};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_product_plan_version")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub plan_id: i64,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: i64,
    pub published_by: Option<i64>,
    pub published_at: Option<PrimitiveDateTime>,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub created_at: OffsetDateTime,
    #[serde(with = "ryframe_application::ports::utc_timestamp")]
    pub updated_at: OffsetDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
