use ryframe_db::entities::permission;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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

#[derive(Debug, Serialize, ToSchema)]
pub struct PermissionTreeNode {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
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
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<permission::Model> for PermissionVo {
    fn from(permission: permission::Model) -> Self {
        Self {
            id: permission.id.to_string(),
            name: permission.name,
            code: permission.code,
            parent_id: permission.parent_id.map(|id| id.to_string()),
            perm_type: permission.perm_type,
            icon: permission.icon,
            sort: permission.sort,
            status: permission.status,
            created_at: permission.created_at,
        }
    }
}

#[derive(Debug)]
pub struct CreatePermissionCommand {
    pub name: String,
    pub code: String,
    pub parent_id: Option<i64>,
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
}

#[derive(Debug)]
pub struct UpdatePermissionCommand {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub parent_id: Option<i64>,
    pub perm_type: PermissionType,
    pub icon: Option<String>,
    pub sort: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PermissionSyncReport {
    pub scanned: usize,
    pub existing: usize,
    pub created: usize,
    pub missing: Vec<String>,
}
