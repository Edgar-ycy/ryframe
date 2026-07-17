use ryframe_db::{entities::menu, repositories::menu_repo::MenuTreeNode as RepoMenuTreeNode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum MenuType {
    #[serde(rename = "M")]
    Directory,
    #[serde(rename = "C")]
    Page,
    #[serde(rename = "F")]
    Action,
}

impl MenuType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Directory => menu::Model::MENU_TYPE_DIR,
            Self::Page => menu::Model::MENU_TYPE_MENU,
            Self::Action => menu::Model::MENU_TYPE_BUTTON,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MenuVo {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub perm_id: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<menu::Model> for MenuVo {
    fn from(menu: menu::Model) -> Self {
        Self {
            id: menu.id.to_string(),
            name: menu.name,
            parent_id: menu.parent_id.map(|id| id.to_string()),
            menu_type: menu.menu_type,
            perm_id: menu.perm_id.map(|id| id.to_string()),
            route_key: menu.route_key,
            icon: menu.icon,
            sort: menu.sort,
            visible: menu.visible,
            status: menu.status,
            remark: menu.remark,
            created_at: menu.created_at,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MenuTreeNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: String,
    pub perm_id: Option<String>,
    pub perm_code: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<MenuTreeNode>,
}

impl From<RepoMenuTreeNode> for MenuTreeNode {
    fn from(node: RepoMenuTreeNode) -> Self {
        Self {
            id: node.id,
            name: node.name,
            parent_id: node.parent_id,
            menu_type: node.menu_type,
            perm_id: node.perm_id,
            perm_code: node.perm_code,
            route_key: node.route_key,
            icon: node.icon,
            sort: node.sort,
            visible: node.visible,
            status: node.status,
            children: node.children.into_iter().map(Self::from).collect(),
        }
    }
}

#[derive(Debug)]
pub struct CreateMenuCommand {
    pub name: String,
    pub parent_id: Option<i64>,
    pub menu_type: MenuType,
    pub perm_id: Option<i64>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
}

#[derive(Debug)]
pub struct UpdateMenuCommand {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub menu_type: MenuType,
    pub perm_id: Option<i64>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
}
