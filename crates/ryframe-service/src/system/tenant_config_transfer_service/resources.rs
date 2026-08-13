use super::*;

pub(super) async fn load_resources_on<C>(
    db: &C,
    tenant_id: &str,
) -> AppResult<TenantConfigPackageResources>
where
    C: ConnectionTrait,
{
    let departments = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .order_by_asc(dept::Column::Id)
        .all(db)
        .await
        .map_err(database_error)?;
    let department_paths = build_department_paths(&departments)?;
    let posts = post::Entity::find()
        .filter(post::Column::TenantId.eq(tenant_id))
        .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?;
    let dict_types = dict_type::Entity::find()
        .filter(dict_type::Column::TenantId.eq(tenant_id))
        .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?;
    let dict_data = dict_data::Entity::find()
        .filter(dict_data::Column::TenantId.eq(tenant_id))
        .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?;
    let configs = config::Entity::find()
        .filter(config::Column::TenantId.eq(tenant_id))
        .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
        .filter(config::Column::Portable.eq(true))
        .all(db)
        .await
        .map_err(database_error)?;
    let permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .map_err(database_error)?;
    let menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?;
    let roles = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .filter(role::Column::IsSuper.eq(0))
        .all(db)
        .await
        .map_err(database_error)?;
    let role_permissions = role_permission::Entity::find()
        .filter(role_permission::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .map_err(database_error)?;
    let role_departments = role_dept::Entity::find()
        .filter(role_dept::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .map_err(database_error)?;

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
    let menu_keys = build_menu_stable_keys(&menus, &permission_codes)?;
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
    let mut resources = TenantConfigPackageResources {
        departments: departments
            .iter()
            .map(|item| {
                Ok(PortableDepartment {
                    path: department_paths
                        .get(&item.id)
                        .cloned()
                        .ok_or_else(|| AppError::Validation("部门路径不存在".into()))?,
                    sort: item.sort,
                    status: item.status.clone(),
                    remark: item.remark.clone(),
                })
            })
            .collect::<AppResult<_>>()?,
        posts: posts
            .into_iter()
            .map(|item| PortablePost {
                code: item.code,
                name: item.name,
                sort: item.sort,
                status: item.status,
                remark: item.remark,
            })
            .collect(),
        dict_types: dict_types
            .into_iter()
            .map(|item| PortableDictType {
                code: item.code,
                name: item.name,
                status: item.status,
                remark: item.remark,
            })
            .collect(),
        dict_data: dict_data
            .into_iter()
            .map(|item| PortableDictData {
                type_code: item.type_code,
                value: item.value,
                label: item.label,
                sort: item.sort,
                status: item.status,
                css_class: item.css_class,
                remark: item.remark,
            })
            .collect(),
        configs: configs
            .into_iter()
            .map(|item| PortableConfig {
                key: item.key,
                name: item.name,
                value: item.value,
                remark: item.remark,
            })
            .collect(),
        permissions: portable_permissions,
        menus: portable_menus,
        roles: roles
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
            .collect(),
    };
    resources.canonicalize();
    Ok(resources)
}

/// 将源租户配置收缩到当前二进制真正支持的 API 权限和页面路由闭包。
///
/// 历史数据库可能仍保留当前版本已经删除的接口权限或页面菜单；它们不能让上传包
/// 自证有效，也不应导致本版本自己导出的包随后被本版本预览阻断。
pub(super) fn filter_exportable_resources(
    mut resources: TenantConfigPackageResources,
    target_catalog: &TenantConfigTargetCatalog,
) -> AppResult<TenantConfigPackageResources> {
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
    resources.canonicalize();
    Ok(resources)
}

pub(super) fn build_department_paths(
    departments: &[dept::Model],
) -> AppResult<BTreeMap<i64, Vec<String>>> {
    fn resolve(
        id: i64,
        by_id: &BTreeMap<i64, &dept::Model>,
        resolved: &mut BTreeMap<i64, Vec<String>>,
        visiting: &mut BTreeSet<i64>,
    ) -> AppResult<Vec<String>> {
        if let Some(path) = resolved.get(&id) {
            return Ok(path.clone());
        }
        if !visiting.insert(id) {
            return Err(AppError::Validation("部门层级存在循环".into()));
        }
        let item = by_id
            .get(&id)
            .ok_or_else(|| AppError::Validation("部门父节点不存在".into()))?;
        let mut path = match item.parent_id {
            Some(parent_id) => resolve(parent_id, by_id, resolved, visiting)?,
            None => Vec::new(),
        };
        path.push(item.name.clone());
        visiting.remove(&id);
        resolved.insert(id, path.clone());
        Ok(path)
    }
    let by_id = departments
        .iter()
        .map(|item| (item.id, item))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = BTreeMap::new();
    for item in departments {
        resolve(item.id, &by_id, &mut resolved, &mut BTreeSet::new())?;
    }
    let mut unique = BTreeSet::new();
    if resolved
        .values()
        .any(|path| !unique.insert(normalize_department_path(path)))
    {
        return Err(AppError::Validation("部门完整路径重复".into()));
    }
    Ok(resolved)
}

pub(super) fn build_menu_stable_keys(
    menus: &[menu::Model],
    permission_codes: &BTreeMap<i64, String>,
) -> AppResult<BTreeMap<i64, String>> {
    fn resolve(
        id: i64,
        by_id: &BTreeMap<i64, &menu::Model>,
        permissions: &BTreeMap<i64, String>,
        resolved: &mut BTreeMap<i64, String>,
        visiting: &mut BTreeSet<i64>,
    ) -> AppResult<String> {
        if let Some(value) = resolved.get(&id) {
            return Ok(value.clone());
        }
        if !visiting.insert(id) {
            return Err(AppError::Validation("菜单层级存在循环".into()));
        }
        let item = by_id
            .get(&id)
            .ok_or_else(|| AppError::Validation("菜单不存在".into()))?;
        let key = if item.menu_type == menu::Model::MENU_TYPE_BUTTON {
            let parent_id = item
                .parent_id
                .ok_or_else(|| AppError::Validation("操作菜单缺少父菜单".into()))?;
            let parent = resolve(parent_id, by_id, permissions, resolved, visiting)?;
            let permission = item
                .perm_id
                .and_then(|id| permissions.get(&id))
                .ok_or_else(|| AppError::Validation("操作菜单缺少权限".into()))?;
            action_menu_key(&parent, permission)
        } else {
            let route_key = item
                .route_key
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| AppError::Validation("目录或页面缺少 route_key".into()))?;
            route_menu_key(route_key)
        };
        visiting.remove(&id);
        resolved.insert(id, key.clone());
        Ok(key)
    }
    let by_id = menus.iter().map(|item| (item.id, item)).collect();
    let mut resolved = BTreeMap::new();
    for item in menus {
        resolve(
            item.id,
            &by_id,
            permission_codes,
            &mut resolved,
            &mut BTreeSet::new(),
        )?;
    }
    let mut unique = BTreeSet::new();
    if resolved
        .values()
        .any(|key| !unique.insert(normalize_stable_key(key)))
    {
        return Err(AppError::Conflict("目标端菜单稳定键重复".into()));
    }
    Ok(resolved)
}
