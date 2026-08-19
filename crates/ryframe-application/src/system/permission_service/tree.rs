use ryframe_db::entities::permission;

use super::PermissionTreeNode;

pub fn build_perm_tree(
    permissions: &[&permission::Model],
    parent_id: Option<i64>,
) -> Vec<PermissionTreeNode> {
    permissions
        .iter()
        .filter(|permission| permission.parent_id == parent_id)
        .map(|permission| PermissionTreeNode {
            id: permission.id.to_string(),
            name: permission.name.clone(),
            code: permission.code.clone(),
            parent_id: permission.parent_id.map(|id| id.to_string()),
            perm_type: permission.perm_type.clone(),
            icon: permission.icon.clone(),
            sort: permission.sort,
            status: permission.status.clone(),
            children: build_perm_tree(permissions, Some(permission.id)),
        })
        .collect()
}
