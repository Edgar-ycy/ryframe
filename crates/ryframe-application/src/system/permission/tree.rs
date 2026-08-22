use std::collections::BTreeMap;

use crate::ports::system::PermissionRecord;

use super::PermissionTreeNode;

pub fn build_perm_tree(permissions: Vec<PermissionRecord>) -> Vec<PermissionTreeNode> {
    let mut children = BTreeMap::<Option<i64>, Vec<PermissionRecord>>::new();
    for permission in permissions {
        children
            .entry(permission.parent_id)
            .or_default()
            .push(permission);
    }
    build_children(&mut children, None)
}

fn build_children(
    children: &mut BTreeMap<Option<i64>, Vec<PermissionRecord>>,
    parent_id: Option<i64>,
) -> Vec<PermissionTreeNode> {
    children
        .remove(&parent_id)
        .unwrap_or_default()
        .into_iter()
        .map(|permission| {
            let nested = build_children(children, Some(permission.id));
            PermissionTreeNode {
                id: permission.id.to_string(),
                name: permission.name,
                code: permission.code,
                parent_id: permission.parent_id.map(|id| id.to_string()),
                perm_type: permission.perm_type,
                icon: permission.icon,
                sort: permission.sort,
                status: permission.status,
                children: nested,
            }
        })
        .collect()
}
