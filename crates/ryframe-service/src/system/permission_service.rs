use ryframe_common::AppResult;
use ryframe_core::LoggedRepo;
use ryframe_db::{PermissionRepository, entities::permission};
use sea_orm::DatabaseConnection;
use serde::Serialize;

#[derive(Debug, Serialize)]
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
    pub children: Vec<PermissionTreeNode>,
}

pub struct PermissionServiceImpl {
    pub perm_repo: LoggedRepo<PermissionRepository>,
}

impl PermissionServiceImpl {
    pub async fn find_tree(
        &self,
        db: &DatabaseConnection,
        perm_type: Option<&str>,
    ) -> AppResult<Vec<PermissionTreeNode>> {
        let all = self.perm_repo.find_all(db).await?;
        let filtered: Vec<&permission::Model> = if let Some(t) = perm_type {
            all.iter().filter(|p| p.perm_type == t).collect()
        } else {
            all.iter().collect()
        };

        let models: Vec<&permission::Model> = filtered;
        Ok(build_perm_tree(&models, None))
    }
}

pub fn build_perm_tree(
    perms: &[&permission::Model],
    parent_id: Option<i64>,
) -> Vec<PermissionTreeNode> {
    perms
        .iter()
        .filter(|p| p.parent_id == parent_id)
        .map(|p| PermissionTreeNode {
            id: p.id.to_string(),
            name: p.name.clone(),
            code: p.code.clone(),
            parent_id: p.parent_id.map(|p| p.to_string()),
            perm_type: p.perm_type.clone(),
            icon: p.icon.clone(),
            sort: p.sort,
            status: p.status.clone(),
            children: build_perm_tree(perms, Some(p.id)),
        })
        .collect()
}
