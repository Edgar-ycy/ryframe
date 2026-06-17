use chrono::{DateTime, Utc};
use ryframe_macro::AutoFill;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize, AutoFill)]
#[sea_orm(table_name = "sys_menu")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[auto_fill(snowflake)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub parent_id: Option<i64>,
    /// 菜单类型: M目录 C菜单 F按钮
    pub menu_type: String,
    pub path: Option<String>,
    pub component: Option<String>,
    /// 路由参数
    pub query: Option<String>,
    /// 权限标识(如 system:user:list)
    pub perms: Option<String>,
    pub icon: Option<String>,
    /// 是否外链
    pub is_frame: bool,
    /// 是否缓存
    pub is_cache: bool,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub del_flag: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Model {
    pub const MENU_TYPE_DIR: &str = "M";
    pub const MENU_TYPE_MENU: &str = "C";
    pub const MENU_TYPE_BUTTON: &str = "F";

    pub const STATUS_DISABLED: &str = "0";
    pub const STATUS_NORMAL: &str = "1";

    pub const DEL_FLAG_NORMAL: &str = "0";
    pub const DEL_FLAG_DELETED: &str = "2";

    pub fn is_enabled(&self) -> bool {
        self.status == Self::STATUS_NORMAL
    }

    /// 是否为目录类型
    pub fn is_dir(&self) -> bool {
        self.menu_type == Self::MENU_TYPE_DIR
    }

    /// 是否为菜单类型
    pub fn is_menu(&self) -> bool {
        self.menu_type == Self::MENU_TYPE_MENU
    }

    /// 是否为按钮类型
    pub fn is_button(&self) -> bool {
        self.menu_type == Self::MENU_TYPE_BUTTON
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::role_menu::Entity")]
    RoleMenu,
}

impl Related<super::role_menu::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RoleMenu.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
