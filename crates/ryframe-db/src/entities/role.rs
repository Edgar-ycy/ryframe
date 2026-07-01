use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_role")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    #[sea_orm(unique)]
    pub code: String,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 数据范围常量
impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";

    pub const DATA_SCOPE_ALL: &str = "1";
    pub const DATA_SCOPE_CUSTOM: &str = "2";
    pub const DATA_SCOPE_DEPT: &str = "3";
    pub const DATA_SCOPE_DEPT_AND_CHILD: &str = "4";
    pub const DATA_SCOPE_SELF: &str = "5";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_role::Entity")]
    UserRole,
    #[sea_orm(has_many = "super::role_permission::Entity")]
    RolePermission,
    #[sea_orm(has_many = "super::role_dept::Entity")]
    RoleDept,
}

impl Related<super::user_role::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserRole.def()
    }
}

impl Related<super::role_permission::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RolePermission.def()
    }
}

impl Related<super::role_dept::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RoleDept.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
