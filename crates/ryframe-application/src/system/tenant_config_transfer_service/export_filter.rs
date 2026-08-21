use super::*;

/// 将源租户配置收缩到当前二进制真正支持的 API 权限和页面路由闭包。
///
/// 历史数据库可能仍保留当前版本已经删除的接口权限或页面菜单；它们不能让上传包
/// 自证有效，也不应导致本版本自己导出的包随后被本版本预览阻断。
pub(super) fn filter_exportable_resources(
    mut resources: TenantConfigPackageResources,
    target_catalog: &TenantConfigTargetCatalog,
    enabled_capabilities: &[CapabilityRequirement],
) -> AppResult<(TenantConfigPackageResources, Vec<CapabilityRequirement>)> {
    let enabled_by_code = enabled_capabilities
        .iter()
        .map(|requirement| (requirement.code.as_str(), requirement))
        .collect::<BTreeMap<_, _>>();
    let disabled_permissions = crate::system::CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| !enabled_by_code.contains_key(descriptor.code))
        .flat_map(|descriptor| descriptor.permission_codes.iter().copied())
        .collect::<BTreeSet<_>>();
    let disabled_routes = crate::system::CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| !enabled_by_code.contains_key(descriptor.code))
        .flat_map(|descriptor| descriptor.route_keys.iter().copied())
        .collect::<BTreeSet<_>>();
    resources
        .permissions
        .retain(|permission| !disabled_permissions.contains(permission.code.as_str()));
    for role in &mut resources.roles {
        role.permission_codes
            .retain(|permission| !disabled_permissions.contains(permission.as_str()));
    }
    resources.menus.retain(|menu| {
        !menu
            .permission_code
            .as_deref()
            .is_some_and(|permission| disabled_permissions.contains(permission))
            && !menu
                .route_key
                .as_deref()
                .is_some_and(|route| disabled_routes.contains(route))
    });

    let permission_types = resources
        .permissions
        .iter()
        .map(|item| (item.code.clone(), item.permission_type.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut allowed_permissions = resources
        .permissions
        .iter()
        .filter(|item| match item.permission_type.as_str() {
            "api" => target_catalog
                .api_permission_codes
                .get(&normalize_stable_key(&item.code))
                .is_some_and(|canonical| canonical == &item.code),
            _ => !target_catalog
                .api_permission_codes
                .contains_key(&normalize_stable_key(&item.code)),
        })
        .map(|item| item.code.clone())
        .collect::<BTreeSet<_>>();
    loop {
        let dangling = resources
            .permissions
            .iter()
            .filter(|item| allowed_permissions.contains(&item.code))
            .filter_map(|item| {
                item.parent_code
                    .as_ref()
                    .filter(|parent| !allowed_permissions.contains(*parent))
                    .map(|_| item.code.clone())
            })
            .collect::<Vec<_>>();
        if dangling.is_empty() {
            break;
        }
        for code in dangling {
            allowed_permissions.remove(&code);
        }
    }
    resources
        .permissions
        .retain(|item| allowed_permissions.contains(&item.code));

    let menu_types = resources
        .menus
        .iter()
        .map(|item| (item.stable_key.clone(), item.menu_type.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut allowed_menus = resources
        .menus
        .iter()
        .filter(|item| {
            let permission_supported = item
                .permission_code
                .as_ref()
                .is_none_or(|code| allowed_permissions.contains(code));
            if !permission_supported {
                return false;
            }
            if matches!(item.menu_type.as_str(), "M" | "C") {
                return item.route_key.as_ref().is_some_and(|route_key| {
                    target_catalog
                        .page_routes
                        .get(&normalize_stable_key(route_key))
                        .is_some_and(|(canonical, menu_type)| {
                            canonical == route_key && menu_type == &item.menu_type
                        })
                });
            }
            item.menu_type == "F"
                && item.permission_code.as_ref().is_some_and(|code| {
                    allowed_permissions.contains(code)
                        && permission_types.get(code).is_some_and(|kind| kind == "api")
                })
        })
        .map(|item| item.stable_key.clone())
        .collect::<BTreeSet<_>>();
    loop {
        let dangling = resources
            .menus
            .iter()
            .filter(|item| allowed_menus.contains(&item.stable_key))
            .filter_map(|item| {
                item.parent_stable_key
                    .as_ref()
                    .filter(|parent| {
                        !allowed_menus.contains(*parent)
                            || menu_types.get(*parent).is_some_and(|kind| kind == "F")
                    })
                    .map(|_| item.stable_key.clone())
            })
            .collect::<Vec<_>>();
        if dangling.is_empty() {
            break;
        }
        for stable_key in dangling {
            allowed_menus.remove(&stable_key);
        }
    }
    resources
        .menus
        .retain(|item| allowed_menus.contains(&item.stable_key));
    for role in &mut resources.roles {
        role.permission_codes
            .retain(|code| allowed_permissions.contains(code));
    }
    let required_codes = crate::system::CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| {
            resources.permissions.iter().any(|permission| {
                descriptor
                    .permission_codes
                    .contains(&permission.code.as_str())
            }) || resources.roles.iter().any(|role| {
                role.permission_codes
                    .iter()
                    .any(|permission| descriptor.permission_codes.contains(&permission.as_str()))
            }) || resources.menus.iter().any(|menu| {
                menu.permission_code
                    .as_deref()
                    .is_some_and(|permission| descriptor.permission_codes.contains(&permission))
                    || menu
                        .route_key
                        .as_deref()
                        .is_some_and(|route| descriptor.route_keys.contains(&route))
            })
        })
        .map(|descriptor| descriptor.code)
        .collect::<BTreeSet<_>>();
    let required_capabilities = required_codes
        .into_iter()
        .map(|code| {
            enabled_by_code
                .get(code)
                .cloned()
                .cloned()
                .ok_or_else(|| AppError::Internal(format!("导出资源引用了未启用能力 {code}")))
        })
        .collect::<AppResult<Vec<_>>>()?;
    resources.canonicalize();
    Ok((resources, required_capabilities))
}
