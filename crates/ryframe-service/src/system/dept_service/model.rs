use ryframe_db::{entities::dept, repositories::dept_repo::DeptTreeNode as RepoDeptTreeNode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct DeptVo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub ancestors: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<dept::Model> for DeptVo {
    fn from(dept: dept::Model) -> Self {
        Self {
            id: dept.id.to_string(),
            name: dept.name,
            parent_id: dept.parent_id.map(|id| id.to_string()),
            ancestors: dept.ancestors,
            sort: dept.sort,
            status: dept.status,
            remark: dept.remark,
            created_at: dept.created_at,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeptTreeNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub sort: i32,
    pub status: String,
    #[schema(no_recursion)]
    pub children: Vec<DeptTreeNode>,
}

impl From<RepoDeptTreeNode> for DeptTreeNode {
    fn from(node: RepoDeptTreeNode) -> Self {
        Self {
            id: node.id,
            name: node.name,
            parent_id: node.parent_id,
            sort: node.sort,
            status: node.status,
            children: node.children.into_iter().map(Self::from).collect(),
        }
    }
}

#[derive(Debug)]
pub struct CreateDeptCommand {
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: i32,
}

#[derive(Debug)]
pub struct UpdateDeptCommand {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: i32,
    pub status: String,
}
