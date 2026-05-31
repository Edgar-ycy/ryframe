use ryframe_common::AppResult;
use ryframe_core::LoggedRepo;
use ryframe_db::{PermissionRepository, entities::permission};
use sea_orm::DatabaseConnection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PermissionTreeNode {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub parent_id: Option<i64>,
    pub perm_type: String,
    pub path: Option<String>,
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
            id: p.id,
            name: p.name.clone(),
            code: p.code.clone(),
            parent_id: p.parent_id,
            perm_type: p.perm_type.clone(),
            path: p.path.clone(),
            icon: p.icon.clone(),
            sort: p.sort,
            status: p.status.clone(),
            children: build_perm_tree(perms, Some(p.id)),
        })
        .collect()
}
