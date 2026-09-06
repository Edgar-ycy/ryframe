use super::*;
use crate::DbResultExt;

mod roles;
mod simple;

pub(crate) async fn apply_resources_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &TenantConfigPackageResources,
    plan_items: &[TenantConfigTransferItemRecord],
    now: DateTime<Utc>,
) -> AppResult<()> {
    let changed = plan_items
        .iter()
        .filter(|item| {
            matches!(
                item.action.as_str(),
                TenantConfigTransferItemRecord::ACTION_CREATE
                    | TenantConfigTransferItemRecord::ACTION_UPDATE
            )
        })
        .map(|item| {
            (
                item.resource_type.clone(),
                normalize_resource_stable_key(&item.resource_type, &item.stable_key),
            )
        })
        .collect::<BTreeSet<_>>();
    let existing_departments = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .db()?;
    let mut department_ids = build_department_paths(&existing_departments)?
        .into_iter()
        .map(|(id, path)| (normalize_department_path(&path), id))
        .collect::<BTreeMap<_, _>>();
    let mut department_ancestors = existing_departments
        .iter()
        .map(|item| (item.id, item.ancestors.clone()))
        .collect::<BTreeMap<_, _>>();
    for item in &resources.departments {
        let stable_key = join_path(&item.path);
        if !changed.contains(&(
            "department".to_owned(),
            normalize_resource_stable_key("department", &stable_key),
        )) {
            continue;
        }
        let parent_path = &item.path[..item.path.len().saturating_sub(1)];
        let parent_id = if parent_path.is_empty() {
            None
        } else {
            Some(
                *department_ids
                    .get(&normalize_department_path(parent_path))
                    .ok_or_else(|| AppError::Conflict("部门父路径不存在".into()))?,
            )
        };
        let ancestors = parent_id.map_or_else(
            || "0".to_owned(),
            |id| {
                department_ancestors
                    .get(&id)
                    .map(|ancestors| format!("{ancestors},{id}"))
                    .unwrap_or_else(|| format!("0,{id}"))
            },
        );
        if let Some(id) = department_ids
            .get(&normalize_department_path(&item.path))
            .copied()
        {
            let mut model = dept::Entity::find_by_id(id)
                .one(transaction)
                .await
                .db()?
                .ok_or_else(|| AppError::Conflict("部门已不存在".into()))?;
            model.name = item.path.last().cloned().unwrap_or_default();
            model.parent_id = parent_id;
            model.ancestors = ancestors.clone();
            model.sort = item.sort;
            model.status = item.status.clone();
            model.remark = item.remark.clone();
            model.del_flag = dept::Model::DEL_FLAG_NORMAL.to_owned();
            model.updated_at = now;
            dept::ActiveModel::from(model)
                .reset_all()
                .update(transaction)
                .await
                .db()?;
            department_ancestors.insert(id, ancestors);
        } else {
            let id = next_id()?;
            dept::ActiveModel::from(dept::Model {
                id,
                tenant_id: tenant_id.to_owned(),
                name: item.path.last().cloned().unwrap_or_default(),
                parent_id,
                ancestors: ancestors.clone(),
                sort: item.sort,
                status: item.status.clone(),
                remark: item.remark.clone(),
                del_flag: dept::Model::DEL_FLAG_NORMAL.to_owned(),
                created_at: now,
                updated_at: now,
            })
            .insert(transaction)
            .await
            .db()?;
            department_ids.insert(normalize_department_path(&item.path), id);
            department_ancestors.insert(id, ancestors);
        }
    }

    simple::upsert_posts(transaction, tenant_id, resources, &changed, now).await?;
    simple::upsert_dict_types(transaction, tenant_id, resources, &changed, now).await?;
    simple::upsert_dict_data(transaction, tenant_id, resources, &changed, now).await?;
    simple::upsert_configs(transaction, tenant_id, resources, &changed, now).await?;
    upsert_permissions(
        transaction,
        tenant_id,
        &resources.permissions,
        &changed,
        now,
    )
    .await?;
    upsert_menus(transaction, tenant_id, &resources.menus, &changed, now).await?;
    roles::upsert_roles_and_relations(
        transaction,
        tenant_id,
        &resources.roles,
        &department_ids,
        &changed,
        now,
    )
    .await
}

async fn save_model<A>(
    transaction: &sea_orm::DatabaseTransaction,
    exists: bool,
    model: A,
) -> AppResult<()>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A as ActiveModelTrait>::Entity: EntityTrait,
    <<A as ActiveModelTrait>::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
    if exists {
        model.reset_all().update(transaction).await.db()?;
    } else {
        model.insert(transaction).await.db()?;
    }
    Ok(())
}

async fn upsert_permissions(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortablePermission],
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let existing = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .db()?;
    let mut by_code = existing
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item))
        .collect::<BTreeMap<_, _>>();
    let source_codes = resources
        .iter()
        .map(|item| normalize_stable_key(&item.code))
        .collect::<BTreeSet<_>>();
    let mut remaining = resources.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for item in remaining {
            if permission_contains_wildcard(&item.code) || is_platform_only_permission(&item.code) {
                return Err(AppError::Validation(
                    "平台专用权限或超级通配权限不能应用".into(),
                ));
            }
            let parent_id = match item.parent_code.as_deref() {
                Some(parent_code) => match by_code.get(&normalize_stable_key(parent_code)) {
                    Some(parent) => Some(parent.id),
                    None if source_codes.contains(&normalize_stable_key(parent_code)) => {
                        deferred.push(item);
                        continue;
                    }
                    None => {
                        return Err(AppError::Conflict(format!(
                            "权限 {} 的父权限不存在",
                            item.code
                        )));
                    }
                },
                None => None,
            };
            if !changed.contains(&("permission".to_owned(), normalize_stable_key(&item.code))) {
                continue;
            }
            let old = by_code.get(&normalize_stable_key(&item.code)).cloned();
            if let Some(old) = &old
                && old.perm_type != item.permission_type
            {
                return Err(AppError::Conflict(format!(
                    "目标权限 {} 的类型与配置包不一致",
                    item.code
                )));
            }
            let model = permission::Model {
                id: old.as_ref().map(|value| value.id).unwrap_or(next_id()?),
                tenant_id: tenant_id.to_owned(),
                name: item.name.clone(),
                code: item.code.clone(),
                parent_id,
                perm_type: old
                    .as_ref()
                    .map(|value| value.perm_type.clone())
                    .unwrap_or_else(|| item.permission_type.clone()),
                icon: item.icon.clone(),
                sort: item.sort,
                status: item.status.clone(),
                created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
                updated_at: now,
            };
            save_model(
                transaction,
                old.is_some(),
                permission::ActiveModel::from(model.clone()),
            )
            .await?;
            by_code.insert(normalize_stable_key(&item.code), model);
        }
        if deferred.len() == before {
            return Err(AppError::Conflict(
                "权限父子层级存在循环或缺少父权限".into(),
            ));
        }
        remaining = deferred;
    }
    Ok(())
}

async fn upsert_menus(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &[PortableMenu],
    changed: &BTreeSet<(String, String)>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let MenuApplyCatalog {
        permission_ids,
        mut by_key,
    } = load_menu_catalog(transaction, tenant_id).await?;
    let source_keys = resources
        .iter()
        .map(|item| normalize_stable_key(&item.stable_key))
        .collect::<BTreeSet<_>>();
    let mut remaining = resources.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for item in remaining {
            let parent_id = match item.parent_stable_key.as_deref() {
                Some(parent_key) => match by_key.get(&normalize_stable_key(parent_key)) {
                    Some(parent) if parent.menu_type == menu::Model::MENU_TYPE_BUTTON => {
                        return Err(AppError::Validation(format!(
                            "菜单 {} 不能将操作菜单作为父菜单",
                            item.stable_key
                        )));
                    }
                    Some(parent) => Some(parent.id),
                    None if source_keys.contains(&normalize_stable_key(parent_key)) => {
                        deferred.push(item);
                        continue;
                    }
                    None => {
                        return Err(AppError::Conflict(format!(
                            "菜单 {} 的父菜单不存在",
                            item.stable_key
                        )));
                    }
                },
                None => None,
            };
            let perm_id = item
                .permission_code
                .as_ref()
                .map(|code| {
                    permission_ids
                        .get(&normalize_stable_key(code))
                        .copied()
                        .ok_or_else(|| AppError::Conflict(format!("菜单引用的权限 {code} 不存在")))
                })
                .transpose()?;
            validate_portable_menu(item, perm_id, parent_id)?;
            if !changed.contains(&("menu".to_owned(), normalize_stable_key(&item.stable_key))) {
                continue;
            }
            let old = by_key.get(&normalize_stable_key(&item.stable_key)).cloned();
            let model = menu::Model {
                id: old.as_ref().map(|value| value.id).unwrap_or(next_id()?),
                tenant_id: tenant_id.to_owned(),
                name: item.name.clone(),
                parent_id,
                menu_type: item.menu_type.clone(),
                perm_id,
                route_key: item.route_key.clone(),
                icon: item.icon.clone(),
                sort: item.sort,
                visible: item.visible,
                status: item.status.clone(),
                remark: item.remark.clone(),
                del_flag: menu::Model::DEL_FLAG_NORMAL.to_owned(),
                created_at: old.as_ref().map(|value| value.created_at).unwrap_or(now),
                updated_at: now,
            };
            save_model(
                transaction,
                old.is_some(),
                menu::ActiveModel::from(model.clone()),
            )
            .await?;
            by_key.insert(normalize_stable_key(&item.stable_key), model);
        }
        if deferred.len() == before {
            return Err(AppError::Conflict(
                "菜单父子层级存在循环或缺少父菜单".into(),
            ));
        }
        remaining = deferred;
    }
    Ok(())
}

fn validate_portable_menu(
    item: &PortableMenu,
    perm_id: Option<i64>,
    parent_id: Option<i64>,
) -> AppResult<()> {
    match item.menu_type.as_str() {
        menu::Model::MENU_TYPE_DIR => {
            if item.route_key.is_none() {
                return Err(AppError::Validation("目录菜单必须声明 route_key".into()));
            }
        }
        menu::Model::MENU_TYPE_MENU => {
            if item.route_key.is_none() || perm_id.is_none() {
                return Err(AppError::Validation(
                    "页面菜单必须声明 route_key 并绑定权限".into(),
                ));
            }
        }
        menu::Model::MENU_TYPE_BUTTON => {
            if item.route_key.is_some() || perm_id.is_none() || parent_id.is_none() {
                return Err(AppError::Validation(
                    "操作菜单必须绑定权限和父菜单，且不能声明 route_key".into(),
                ));
            }
        }
        _ => return Err(AppError::Validation("配置包菜单类型不受支持".into())),
    }
    Ok(())
}

struct MenuApplyCatalog {
    permission_ids: BTreeMap<String, i64>,
    by_key: BTreeMap<String, menu::Model>,
}

async fn load_menu_catalog(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
) -> AppResult<MenuApplyCatalog> {
    let permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .db()?;
    let permission_ids = permissions
        .into_iter()
        .map(|item| (normalize_stable_key(&item.code), item.id))
        .collect::<BTreeMap<_, _>>();
    let existing = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .db()?;
    let existing_permission_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .db()?
        .into_iter()
        .map(|item| (item.id, item.code))
        .collect::<BTreeMap<_, _>>();
    let stable_keys = build_menu_stable_keys(&existing, &existing_permission_codes)?;
    let by_key = existing
        .into_iter()
        .filter_map(|item| {
            stable_keys
                .get(&item.id)
                .cloned()
                .map(|key| (normalize_stable_key(&key), item))
        })
        .collect::<BTreeMap<_, _>>();
    Ok(MenuApplyCatalog {
        permission_ids,
        by_key,
    })
}
