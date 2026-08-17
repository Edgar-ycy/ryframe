use chrono::{DateTime, Utc};
use ryframe_kernel::AppError;
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

impl TryFrom<&str> for MenuType {
    type Error = AppError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "M" => Ok(Self::Directory),
            "C" => Ok(Self::Page),
            "F" => Ok(Self::Action),
            _ => {
                tracing::error!(menu_type = value, "服务层返回了未识别的菜单类型");
                Err(AppError::Internal("菜单数据包含未识别的 menu_type".into()))
            }
        }
    }
}

/// 菜单响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuVo {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: MenuType,
    pub perm_id: Option<String>,
    pub route_key: Option<String>,
    pub icon: Option<String>,
    pub sort: i32,
    pub visible: bool,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<ServiceMenuVo> for MenuVo {
    type Error = AppError;

    fn try_from(value: ServiceMenuVo) -> Result<Self, Self::Error> {
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
        Ok(Self {
            id,
            name,
            parent_id,
            menu_type: MenuType::try_from(menu_type.as_str())?,
            perm_id,
            route_key,
            icon,
            sort,
            visible,
            status,
            remark,
            created_at,
        })
    }
}

/// 菜单树节点。
#[derive(Debug, Serialize, ToSchema)]
pub struct MenuTreeNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub menu_type: MenuType,
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

impl TryFrom<ServiceMenuTreeNode> for MenuTreeNode {
    type Error = AppError;

    fn try_from(value: ServiceMenuTreeNode) -> Result<Self, Self::Error> {
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
        let children = children
            .into_iter()
            .map(Self::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id,
            name,
            parent_id,
            menu_type: MenuType::try_from(menu_type.as_str())?,
            perm_id,
            perm_code,
            route_key,
            icon,
            sort,
            visible,
            status,
            children,
        })
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

impl TryFrom<&str> for PermissionType {
    type Error = AppError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "api" => Ok(Self::Api),
            "menu" => Ok(Self::Menu),
            _ => {
                tracing::error!(perm_type = value, "服务层返回了未识别的权限类型");
                Err(AppError::Internal("权限数据包含未识别的 perm_type".into()))
            }
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
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<PermissionTreeNode>,
}

impl TryFrom<ServicePermissionTreeNode> for PermissionTreeNode {
    type Error = AppError;

    fn try_from(value: ServicePermissionTreeNode) -> Result<Self, Self::Error> {
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
        let children = children
            .into_iter()
            .map(Self::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id,
            name,
            code,
            parent_id,
            perm_type: PermissionType::try_from(perm_type.as_str())?,
            icon,
            sort,
            status,
            children,
        })
    }
}

/// 权限详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionVo {
    pub id: String,
    pub name: String,
    pub code: String,
    pub parent_id: Option<String>,
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<ServicePermissionVo> for PermissionVo {
    type Error = AppError;

    fn try_from(value: ServicePermissionVo) -> Result<Self, Self::Error> {
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
        Ok(Self {
            id,
            name,
            code,
            parent_id,
            perm_type: PermissionType::try_from(perm_type.as_str())?,
            icon,
            sort,
            status,
            created_at,
        })
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
