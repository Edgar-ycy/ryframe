use super::*;

pub(super) fn roles(
    assigned_roles: Vec<crate::ports::authorization::DiagnosticRoleRecord>,
    enabled_roles: &[IdentityRoleRecord],
    final_access_enabled: bool,
) -> Vec<AuthorizationDiagnosticRoleVo> {
    let enabled_role_ids = enabled_roles
        .iter()
        .map(|role| role.id)
        .collect::<HashSet<_>>();
    assigned_roles
        .into_iter()
        .map(|role| AuthorizationDiagnosticRoleVo {
            id: role.id.to_string(),
            name: role.name,
            code: role.code,
            status: role.status,
            data_scope: data_scope_key(&DataScope::from_db_value(&role.data_scope)).to_owned(),
            is_super: role.is_super,
            participates: final_access_enabled && enabled_role_ids.contains(&role.id),
        })
        .collect::<Vec<_>>()
}

pub(super) fn permissions(
    permission_sources: BTreeMap<String, PermissionSource>,
    final_access_enabled: bool,
) -> Vec<AuthorizationDiagnosticPermissionVo> {
    permission_sources
        .into_values()
        .filter_map(|source| {
            source
                .permission
                .map(|permission| AuthorizationDiagnosticPermissionVo {
                    id: permission.id.to_string(),
                    name: permission.name,
                    code: permission.code,
                    source_roles: source.roles.into_iter().collect(),
                    effective: final_access_enabled,
                })
        })
        .collect::<Vec<_>>()
}

pub(super) fn menus(
    all_menus: Vec<DiagnosticMenuRecord>,
    permission_by_id: &HashMap<i64, &DiagnosticPermissionRecord>,
    accessible_ids: &HashSet<i64>,
    tenant_available: bool,
    user_enabled: bool,
) -> (Vec<AuthorizationDiagnosticMenuVo>, bool) {
    let mut invalid_menu_permission = false;
    let menus = all_menus
        .into_iter()
        .filter(|menu| !menu.is_button())
        .map(|menu| {
            let permission = menu
                .perm_id
                .and_then(|permission_id| permission_by_id.get(&permission_id).copied());
            if menu.perm_id.is_some() && permission.is_none() {
                invalid_menu_permission = true;
            }
            let accessible = accessible_ids.contains(&menu.id);
            let inaccessible_reason = menu_inaccessible_reason(
                &menu,
                permission,
                tenant_available,
                user_enabled,
                accessible,
            );
            AuthorizationDiagnosticMenuVo {
                id: menu.id.to_string(),
                parent_id: menu.parent_id.map(|id| id.to_string()),
                name: menu.name,
                route_key: menu.route_key,
                permission_code: permission.map(|permission| permission.code.clone()),
                status: menu.status,
                configured_visible: menu.visible,
                accessible,
                visible_in_navigation: accessible && menu.visible,
                inaccessible_reason,
            }
        })
        .collect::<Vec<_>>();

    (menus, invalid_menu_permission)
}

pub(super) fn data_scope_sources(
    roles: &[IdentityRoleRecord],
) -> Vec<AuthorizationDiagnosticDataScopeSourceVo> {
    roles
        .iter()
        .map(|role| AuthorizationDiagnosticDataScopeSourceVo {
            role_code: role.code.clone(),
            scope: data_scope_key(&DataScope::from_db_value(&role.data_scope)).to_owned(),
        })
        .collect::<Vec<_>>()
}
