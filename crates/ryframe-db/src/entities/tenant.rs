use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_tenant")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub status: String,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
    /// 每当必须使租户会话失效时递增。
    pub session_version: i32,
    /// 租户级授权规则版本，角色权限、菜单权限或部门层级变化时递增。
    pub authorization_epoch: i32,
    /// 部门、岗位、字典、可迁移参数、权限、菜单或角色关系变化时递增。
    pub configuration_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";

    pub fn is_available(&self, now: DateTime<Utc>) -> bool {
        self.status == Self::STATUS_NORMAL && self.expire_at.is_none_or(|expire_at| expire_at > now)
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
