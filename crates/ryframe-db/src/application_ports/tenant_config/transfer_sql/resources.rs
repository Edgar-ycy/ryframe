use super::*;
use crate::DbResultExt;

mod projection;
use projection::{project_menus, project_permissions, project_roles};

pub(crate) async fn load_resources_on<C>(
    db: &C,
    tenant_id: &str,
) -> AppResult<TenantConfigPackageResources>
where
    C: ConnectionTrait,
{
    let departments = load_departments(db, tenant_id).await?;
    let department_paths = build_department_paths(&departments)?;
    let ResourceSnapshot {
        posts,
        dict_types,
        dict_data,
        configs,
        permissions,
        menus,
        roles,
        role_permissions,
        role_departments,
    } = load_snapshot(db, tenant_id).await?;
    let permission_projection = project_permissions(&permissions)?;
    let portable_menus = project_menus(&menus, &permission_projection)?;
    let portable_roles = project_roles(
        roles,
        role_permissions,
        role_departments,
        &permission_projection,
        &department_paths,
    )?;
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
        permissions: permission_projection.records,
        menus: portable_menus,
        roles: portable_roles,
    };
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

struct ResourceSnapshot {
    posts: Vec<post::Model>,
    dict_types: Vec<dict_type::Model>,
    dict_data: Vec<dict_data::Model>,
    configs: Vec<config::Model>,
    permissions: Vec<permission::Model>,
    menus: Vec<menu::Model>,
    roles: Vec<role::Model>,
    role_permissions: Vec<role_permission::Model>,
    role_departments: Vec<role_dept::Model>,
}

async fn load_departments<C: ConnectionTrait>(
    db: &C,
    tenant_id: &str,
) -> AppResult<Vec<dept::Model>> {
    dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .order_by_asc(dept::Column::Id)
        .all(db)
        .await
        .db()
}

async fn load_snapshot<C: ConnectionTrait>(db: &C, tenant_id: &str) -> AppResult<ResourceSnapshot> {
    let posts = post::Entity::find()
        .filter(post::Column::TenantId.eq(tenant_id))
        .filter(post::Column::DelFlag.eq(post::SOFT_DELETE_ACTIVE))
        .all(db)
        .await
        .db()?;
    let dict_types = dict_type::Entity::find()
        .filter(dict_type::Column::TenantId.eq(tenant_id))
        .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .db()?;
    let dict_data = dict_data::Entity::find()
        .filter(dict_data::Column::TenantId.eq(tenant_id))
        .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .db()?;
    let configs = config::Entity::find()
        .filter(config::Column::TenantId.eq(tenant_id))
        .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
        .filter(config::Column::Portable.eq(true))
        .all(db)
        .await
        .db()?;
    let permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .db()?;
    let menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .db()?;
    let roles = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .filter(role::Column::IsSuper.eq(0))
        .all(db)
        .await
        .db()?;
    let role_permissions = role_permission::Entity::find()
        .filter(role_permission::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .db()?;
    let role_departments = role_dept::Entity::find()
        .filter(role_dept::Column::TenantId.eq(tenant_id))
        .all(db)
        .await
        .db()?;

    Ok(ResourceSnapshot {
        posts,
        dict_types,
        dict_data,
        configs,
        permissions,
        menus,
        roles,
        role_permissions,
        role_departments,
    })
}
