use super::*;

pub(super) struct PermissionProjection {
    pub(super) records: Vec<PortablePermission>,
    codes: BTreeMap<i64, String>,
    portable_codes: BTreeMap<i64, String>,
    ids: BTreeSet<i64>,
}

pub(super) fn project_permissions(
    permissions: &[permission::Model],
) -> AppResult<PermissionProjection> {
    let permission_codes = permissions
        .iter()
        .map(|item| (item.id, item.code.clone()))
        .collect::<BTreeMap<_, _>>();
    // 系统租户含平台专用与超级通配权限；导出时先求可迁移权限闭包，避免产生目标端
    // 必然拒绝或存在悬空父引用的配置包。
    let mut portable_permission_ids = permissions
        .iter()
        .filter(|item| {
            !permission_contains_wildcard(&item.code) && !is_platform_only_permission(&item.code)
        })
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    loop {
        let dangling = permissions
            .iter()
            .filter(|item| portable_permission_ids.contains(&item.id))
            .filter_map(|item| {
                item.parent_id
                    .filter(|parent_id| !portable_permission_ids.contains(parent_id))
                    .map(|_| item.id)
            })
            .collect::<Vec<_>>();
        if dangling.is_empty() {
            break;
        }
        for permission_id in dangling {
            portable_permission_ids.remove(&permission_id);
        }
    }
    let portable_permission_codes = permission_codes
        .iter()
        .filter(|(id, _)| portable_permission_ids.contains(id))
        .map(|(id, code)| (*id, code.clone()))
        .collect::<BTreeMap<_, _>>();
    let portable_permissions = permissions
        .iter()
        .filter(|item| portable_permission_ids.contains(&item.id))
        .map(|item| {
            Ok(PortablePermission {
                code: item.code.clone(),
                name: item.name.clone(),
                parent_code: item
                    .parent_id
                    .map(|id| {
                        portable_permission_codes
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| AppError::Validation("权限父节点不存在".into()))
                    })
                    .transpose()?,
                permission_type: item.perm_type.clone(),
                icon: item.icon.clone(),
                sort: item.sort,
                status: item.status.clone(),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(PermissionProjection {
        records: portable_permissions,
        codes: permission_codes,
        portable_codes: portable_permission_codes,
        ids: portable_permission_ids,
    })
}

pub(super) fn project_menus(
    menus: &[menu::Model],
    projection: &PermissionProjection,
) -> AppResult<Vec<PortableMenu>> {
    let permission_codes = &projection.codes;
    let portable_permission_ids = &projection.ids;
    let portable_permission_codes = &projection.portable_codes;
    let menu_keys = build_menu_stable_keys(menus, permission_codes)?;
    let mut portable_menu_ids = menus
        .iter()
        .filter(|item| match item.menu_type.as_str() {
            menu::Model::MENU_TYPE_DIR => item
                .perm_id
                .is_none_or(|permission_id| portable_permission_ids.contains(&permission_id)),
            menu::Model::MENU_TYPE_MENU | menu::Model::MENU_TYPE_BUTTON => item
                .perm_id
                .is_some_and(|permission_id| portable_permission_ids.contains(&permission_id)),
            _ => false,
        })
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    loop {
        let dangling = menus
            .iter()
            .filter(|item| portable_menu_ids.contains(&item.id))
            .filter_map(|item| {
                item.parent_id
                    .filter(|parent_id| !portable_menu_ids.contains(parent_id))
                    .map(|_| item.id)
            })
            .collect::<Vec<_>>();
        if dangling.is_empty() {
            break;
        }
        for menu_id in dangling {
            portable_menu_ids.remove(&menu_id);
        }
    }
    let portable_menus = menus
        .iter()
        .filter(|item| portable_menu_ids.contains(&item.id))
        .map(|item| {
            Ok(PortableMenu {
                stable_key: menu_keys
                    .get(&item.id)
                    .cloned()
                    .ok_or_else(|| AppError::Validation("菜单稳定键解析失败".into()))?,
                parent_stable_key: item
                    .parent_id
                    .map(|id| {
                        menu_keys
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| AppError::Validation("菜单父节点不存在".into()))
                    })
                    .transpose()?,
                name: item.name.clone(),
                menu_type: item.menu_type.clone(),
                permission_code: item
                    .perm_id
                    .map(|id| {
                        portable_permission_codes
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| AppError::Validation("菜单权限不存在".into()))
                    })
                    .transpose()?,
                route_key: item.route_key.clone(),
                icon: item.icon.clone(),
                sort: item.sort,
                visible: item.visible,
                status: item.status.clone(),
                remark: item.remark.clone(),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(portable_menus)
}

pub(super) fn project_roles(
    roles: Vec<role::Model>,
    role_permissions: Vec<role_permission::Model>,
    role_departments: Vec<role_dept::Model>,
    projection: &PermissionProjection,
    department_paths: &BTreeMap<i64, Vec<String>>,
) -> AppResult<Vec<PortableRole>> {
    let portable_permission_ids = &projection.ids;
    let portable_permission_codes = &projection.portable_codes;
    let role_ids = roles.iter().map(|item| item.id).collect::<BTreeSet<_>>();
    let mut permissions_by_role = BTreeMap::<i64, Vec<String>>::new();
    for relation in role_permissions {
        if role_ids.contains(&relation.role_id)
            && portable_permission_ids.contains(&relation.perm_id)
        {
            permissions_by_role
                .entry(relation.role_id)
                .or_default()
                .push(
                    portable_permission_codes
                        .get(&relation.perm_id)
                        .cloned()
                        .ok_or_else(|| AppError::Validation("角色权限引用不存在".into()))?,
                );
        }
    }
    let mut departments_by_role = BTreeMap::<i64, Vec<Vec<String>>>::new();
    for relation in role_departments {
        if role_ids.contains(&relation.role_id) {
            departments_by_role
                .entry(relation.role_id)
                .or_default()
                .push(
                    department_paths
                        .get(&relation.dept_id)
                        .cloned()
                        .ok_or_else(|| AppError::Validation("角色部门引用不存在".into()))?,
                );
        }
    }
    Ok(roles
        .into_iter()
        .map(|item| PortableRole {
            code: item.code,
            name: item.name,
            data_scope: item.data_scope,
            status: item.status,
            sort: item.sort,
            remark: item.remark,
            permission_codes: permissions_by_role.remove(&item.id).unwrap_or_default(),
            custom_department_paths: departments_by_role.remove(&item.id).unwrap_or_default(),
        })
        .collect())
}
