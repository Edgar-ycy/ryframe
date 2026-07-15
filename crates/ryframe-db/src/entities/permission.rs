use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_permission")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<i64>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::role_permission::Entity")]
    RolePermission,
    #[sea_orm(has_many = "super::menu::Entity")]
    Menu,
}

impl Related<super::role_permission::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RolePermission.def()
    }
}

impl Related<super::menu::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Menu.def()
    }
}

impl Related<super::role::Entity> for Entity {
    fn to() -> RelationDef {
        super::role_permission::Relation::Role.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::role_permission::Relation::Permission.def().rev())
    }
}

impl ActiveModelBehavior for ActiveModel {}
