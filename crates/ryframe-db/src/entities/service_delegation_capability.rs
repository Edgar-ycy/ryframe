use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 委托令牌被允许调用的编译期注册查询能力白名单。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_service_delegation_capability")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub delegation_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub capability_key: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
