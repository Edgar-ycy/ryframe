use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub tenant_id: String,
    #[sea_orm(unique)]
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub auth_version: i32,
    pub dept_id: Option<i64>,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    #[auto_fill(skip)]
    pub login_date: Option<DateTime<Utc>>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";
    pub const STATUS_LOCKED: &str = "2";
    pub const STATUS_PENDING_ACTIVATION: &str = "pending_activation";
    pub const STATUS_MUST_RESET_PASSWORD: &str = "must_reset_password";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_role::Entity")]
    UserRole,
    #[sea_orm(
        belongs_to = "super::dept::Entity",
        from = "Column::DeptId",
        to = "super::dept::Column::Id"
    )]
    Dept,
}

impl Related<super::user_role::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserRole.def()
    }
}

impl Related<super::dept::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Dept.def()
    }
}

impl Related<super::role::Entity> for Entity {
    fn to() -> RelationDef {
        super::user_role::Relation::Role.def()
    }

    fn via() -> Option<RelationDef> {
        Some(super::user_role::Relation::User.def().rev())
    }
}

impl ActiveModelBehavior for ActiveModel {}
