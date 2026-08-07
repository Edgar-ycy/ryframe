use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use ryframe_service::system::{
    MenuTreeNode as ServiceMenuTreeNode, MenuType as ServiceMenuType, MenuVo as ServiceMenuVo,
    PermissionSyncReport as ServicePermissionSyncReport,
    PermissionTreeNode as ServicePermissionTreeNode, PermissionType as ServicePermissionType,
    PermissionVo as ServicePermissionVo,
};

/// 菜单类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum MenuType {
    #[serde(rename = "M")]
    Directory,
    #[serde(rename = "C")]
    Page,
    #[serde(rename = "F")]
    Action,
}

impl From<MenuType> for ServiceMenuType {
    fn from(value: MenuType) -> Self {
        match value {
            MenuType::Directory => Self::Directory,
            MenuType::Page => Self::Page,
            MenuType::Action => Self::Action,
        }
    }
}

/// 菜单响应。
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
    pub created_at: DateTime<Utc>,
}

impl From<ServiceMenuVo> for MenuVo {
    fn from(value: ServiceMenuVo) -> Self {
        let ServiceMenuVo {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            route_key,
            icon,
            sort,
            visible,
            status,
            remark,
            created_at,
        } = value;
        Self {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            route_key,
            icon,
            sort,
            visible,
            status,
            remark,
            created_at,
        }
    }
}

/// 菜单树节点。
#[derive(Debug, Serialize, ToSchema)]
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

impl From<ServiceMenuTreeNode> for MenuTreeNode {
    fn from(value: ServiceMenuTreeNode) -> Self {
        let ServiceMenuTreeNode {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            perm_code,
            route_key,
            icon,
            sort,
            visible,
            status,
            children,
        } = value;
        Self {
            id,
            name,
            parent_id,
            menu_type,
            perm_id,
            perm_code,
            route_key,
            icon,
            sort,
            visible,
            status,
            children: children.into_iter().map(Self::from).collect(),
        }
    }
}

/// 权限类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionType {
    Api,
    Menu,
}

impl PermissionType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Menu => "menu",
        }
    }
}

impl From<PermissionType> for ServicePermissionType {
    fn from(value: PermissionType) -> Self {
        match value {
            PermissionType::Api => Self::Api,
            PermissionType::Menu => Self::Menu,
        }
    }
}

/// 权限树节点。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionTreeNode {
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<PermissionTreeNode>,
}

impl From<ServicePermissionTreeNode> for PermissionTreeNode {
    fn from(value: ServicePermissionTreeNode) -> Self {
        let ServicePermissionTreeNode {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            children,
        } = value;
        Self {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            children: children.into_iter().map(Self::from).collect(),
        }
    }
}

/// 权限详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: String,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl From<ServicePermissionVo> for PermissionVo {
    fn from(value: ServicePermissionVo) -> Self {
        let ServicePermissionVo {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            created_at,
        } = value;
        Self {
            id,
            name,
            code,
            parent_id,
            perm_type,
            icon,
            sort,
            status,
            created_at,
        }
    }
}

/// 权限同步结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionSyncReport {
    pub scanned: usize,
    pub existing: usize,
    pub created: usize,
    pub missing: Vec<String>,
}

impl From<ServicePermissionSyncReport> for PermissionSyncReport {
    fn from(value: ServicePermissionSyncReport) -> Self {
        let ServicePermissionSyncReport {
            scanned,
            existing,
            created,
            missing,
        } = value;
        Self {
            scanned,
            existing,
            created,
            missing,
        }
    }
}
