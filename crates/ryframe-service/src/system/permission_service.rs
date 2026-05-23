use ryframe_common::AppResult;
use ryframe_db::entities::permission;
use ryframe_db::PermissionRepository;
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
    pub perm_repo: PermissionRepository,
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

fn build_perm_tree(
    perms: &[&permission::Model],
    parent_id: Option<i64>,
) -> Vec<PermissionTreeNode> {
    perms.iter()
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

#[cfg(test)]
mod tests {
    use super::*;
    use ryframe_db::entities::permission;

    fn make_perm(id: i64, name: &str, code: &str, parent_id: Option<i64>) -> permission::Model {
        let now = chrono::Utc::now();
        permission::Model {
            id, name: name.into(), code: code.into(), parent_id, perm_type: "api".into(),
            path: None, icon: None, sort: 0, status: "1".into(),
            created_at: now, updated_at: now,
        }
    }

    #[test]
    fn test_build_perm_tree() {
        // 空列表
        let tree = build_perm_tree(&Vec::<&permission::Model>::new(), None);
        assert!(tree.is_empty());

        // 单层
        let p1 = make_perm(1, "用户列表", "system:user:list", None);
        let p2 = make_perm(2, "角色列表", "system:role:list", None);
        let perms = vec![&p1, &p2];
        let tree = build_perm_tree(&perms, None);
        assert_eq!(tree.len(), 2);

        // 嵌套
        let parent = make_perm(10, "系统管理", "system", None);
        let child1 = make_perm(11, "用户管理", "system:user", Some(10));
        let child2 = make_perm(12, "角色管理", "system:role", Some(10));
        let perms = vec![&parent, &child1, &child2];
        let tree = build_perm_tree(&perms, None);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].name, "用户管理");
    }
}
