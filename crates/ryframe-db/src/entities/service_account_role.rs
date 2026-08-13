use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 服务账号与普通角色之间的租户内授权关系。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_service_account_role")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub tenant_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub account_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub role_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
