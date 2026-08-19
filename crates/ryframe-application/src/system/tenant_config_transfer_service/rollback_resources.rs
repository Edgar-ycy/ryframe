use super::*;

pub(super) async fn ensure_rollback_references_safe(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
) -> AppResult<()> {
    let created = load_created_item_keys(transaction, tenant_id, transfer_id).await?;
    let created_roles = created_keys(&created, "role").collect::<BTreeSet<_>>();
    let extra_role_ids = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_roles.contains(&normalize_stable_key(&item.code)))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    if !extra_role_ids.is_empty()
        && user_role::Entity::find()
            .filter(user_role::Column::TenantId.eq(tenant_id))
            .filter(user_role::Column::RoleId.is_in(extra_role_ids))
            .count(transaction)
            .await
            .map_err(database_error)?
            > 0
    {
        return Err(AppError::Conflict(
            "应用创建的角色已经分配给用户，不能自动回滚".into(),
        ));
    }
    let created_departments = created_keys(&created, "department").collect::<BTreeSet<_>>();
    let current_models = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let paths = build_department_paths(&current_models)?;
    let extra_dept_ids = paths
        .into_iter()
        .filter_map(|(id, path)| {
            created_departments
                .contains(&normalize_resource_stable_key(
                    "department",
                    &join_path(&path),
                ))
                .then_some(id)
        })
        .collect::<Vec<_>>();
    if !extra_dept_ids.is_empty()
        && user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::DeptId.is_in(extra_dept_ids.clone()))
            .count(transaction)
            .await
            .map_err(database_error)?
            > 0
    {
        return Err(AppError::Conflict(
            "应用创建的部门已经被用户引用，不能自动回滚".into(),
        ));
    }
    if !extra_dept_ids.is_empty() {
        let migrated_role_keys = tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .filter(tenant_config_transfer_item::Column::ResourceType.eq("role"))
            .filter(tenant_config_transfer_item::Column::Action.is_in([
                tenant_config_transfer_item::Model::ACTION_CREATE,
                tenant_config_transfer_item::Model::ACTION_UPDATE,
            ]))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|item| normalize_stable_key(&item.stable_key))
            .collect::<BTreeSet<_>>();
        let migrated_role_ids = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .filter_map(|item| {
                migrated_role_keys
                    .contains(&normalize_stable_key(&item.code))
                    .then_some(item.id)
            })
            .collect::<BTreeSet<_>>();
        let unexpected_reference = role_dept::Entity::find()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::DeptId.is_in(extra_dept_ids))
            .all(transaction)
            .await
            .map_err(database_error)?
            .into_iter()
            .any(|relation| !migrated_role_ids.contains(&relation.role_id));
        if unexpected_reference {
            return Err(AppError::Conflict(
                "应用创建的部门已经被迁移范围外的角色数据范围引用，不能自动回滚".into(),
            ));
        }
    }
    Ok(())
}

async fn load_created_item_keys(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    transfer_id: i64,
) -> AppResult<BTreeMap<String, BTreeSet<String>>> {
    let items = tenant_config_transfer_item::Entity::find()
        .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
        .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
        .filter(
            tenant_config_transfer_item::Column::Action
                .eq(tenant_config_transfer_item::Model::ACTION_CREATE),
        )
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut created = BTreeMap::<String, BTreeSet<String>>::new();
    for item in items {
        let normalized_key = normalize_resource_stable_key(&item.resource_type, &item.stable_key);
        created
            .entry(item.resource_type)
            .or_default()
            .insert(normalized_key);
    }
    Ok(created)
}

fn created_keys<'a>(
    created: &'a BTreeMap<String, BTreeSet<String>>,
    resource_type: &str,
) -> impl Iterator<Item = String> + 'a {
    created.get(resource_type).into_iter().flatten().cloned()
}

pub(super) async fn restore_snapshot_in_transaction(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    snapshot: &TenantConfigPackageResources,
    transfer_id: i64,
    target_catalog: &TenantConfigTargetCatalog,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let current = load_resources_on(transaction, tenant_id).await?;
    let mut descriptions = Vec::new();
    compare_resources(
        snapshot,
        &current,
        &target_catalog.page_routes,
        &target_catalog.api_permission_codes,
        &mut descriptions,
    )?;
    if descriptions.iter().any(|item| {
        matches!(
            item.action,
            tenant_config_transfer_item::Model::ACTION_BLOCKED
                | tenant_config_transfer_item::Model::ACTION_CONFLICT
        )
    }) {
        return Err(AppError::Conflict(
            "目标权限或路由目录已经变化，配置快照不能完整回滚".into(),
        ));
    }
    let plan_items = descriptions
        .into_iter()
        .map(|item| tenant_config_transfer_item::Model {
            id: 0,
            tenant_id: tenant_id.to_owned(),
            transfer_id: 0,
            resource_type: item.resource_type.to_owned(),
            stable_key: item.stable_key,
            display_name: item.display_name,
            action: item.action.to_owned(),
            outcome: tenant_config_transfer_item::Model::OUTCOME_PENDING.to_owned(),
            detail_code: item.detail_code.map(str::to_owned),
            detail: item.detail,
            created_at: now,
            updated_at: now,
        })
        .collect::<Vec<_>>();
    apply_resources_in_transaction(transaction, tenant_id, snapshot, &plan_items, now).await?;
    let created = load_created_item_keys(transaction, tenant_id, transfer_id).await?;

    let created_role_codes = created_keys(&created, "role").collect::<BTreeSet<_>>();
    let extra_roles = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::IsSuper.eq(0))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_role_codes.contains(&normalize_stable_key(&item.code)))
        .collect::<Vec<_>>();
    for item in extra_roles {
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::RoleId.eq(item.id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        role_dept::Entity::delete_many()
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .filter(role_dept::Column::RoleId.eq(item.id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        let mut model = item;
        model.del_flag = role::Model::DEL_FLAG_DELETED.to_owned();
        model.updated_at = now;
        role::ActiveModel::from(model)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
    }

    let permission_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|item| (item.id, item.code))
        .collect::<BTreeMap<_, _>>();
    let current_menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let current_menu_keys = build_menu_stable_keys(&current_menus, &permission_codes)?;
    let created_menu_keys = created_keys(&created, "menu").collect::<BTreeSet<_>>();
    for mut item in current_menus {
        if current_menu_keys
            .get(&item.id)
            .is_some_and(|key| created_menu_keys.contains(&normalize_stable_key(key)))
        {
            item.del_flag = menu::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            menu::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }

    let created_permission_codes = created_keys(&created, "permission").collect::<BTreeSet<_>>();
    let extra_permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter(|item| created_permission_codes.contains(&normalize_stable_key(&item.code)))
        .collect::<Vec<_>>();
    for item in extra_permissions {
        let referenced = role_permission::Entity::find()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::PermId.eq(item.id))
            .count(transaction)
            .await
            .map_err(database_error)?
            + menu::Entity::find()
                .filter(menu::Column::TenantId.eq(tenant_id))
                .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
                .filter(menu::Column::PermId.eq(item.id))
                .count(transaction)
                .await
                .map_err(database_error)?;
        if referenced > 0 {
            return Err(AppError::Conflict(
                "应用创建的权限仍被引用，不能自动回滚".into(),
            ));
        }
        permission::Entity::delete_by_id(item.id)
            .exec(transaction)
            .await
            .map_err(database_error)?;
    }

    soft_delete_simple_extras(transaction, tenant_id, &created, now).await?;
    soft_delete_department_extras(transaction, tenant_id, &created, now).await
}

async fn soft_delete_simple_extras(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    created: &BTreeMap<String, BTreeSet<String>>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let post_codes = created_keys(created, "post").collect::<BTreeSet<_>>();
    for mut item in post::Entity::find()
        .filter(post::Column::TenantId.eq(tenant_id))
        .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if post_codes.contains(&normalize_stable_key(&item.code)) {
            item.del_flag = post::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            post::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let data_keys = created_keys(created, "dict_data").collect::<BTreeSet<_>>();
    for mut item in dict_data::Entity::find()
        .filter(dict_data::Column::TenantId.eq(tenant_id))
        .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        let stable_key = format!("{}:{}:{}", item.type_code.len(), item.type_code, item.value);
        if data_keys.contains(&normalize_stable_key(&stable_key)) {
            item.del_flag = dict_data::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            dict_data::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let type_codes = created_keys(created, "dict_type").collect::<BTreeSet<_>>();
    for mut item in dict_type::Entity::find()
        .filter(dict_type::Column::TenantId.eq(tenant_id))
        .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if type_codes.contains(&normalize_stable_key(&item.code)) {
            item.del_flag = dict_type::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            dict_type::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    let config_keys = created_keys(created, "config").collect::<BTreeSet<_>>();
    for mut item in config::Entity::find()
        .filter(config::Column::TenantId.eq(tenant_id))
        .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
        .filter(config::Column::Portable.eq(true))
        .all(transaction)
        .await
        .map_err(database_error)?
    {
        if config_keys.contains(&normalize_stable_key(&item.key)) {
            item.del_flag = config::Model::DEL_FLAG_DELETED.to_owned();
            item.updated_at = now;
            config::ActiveModel::from(item)
                .reset_all()
                .update(transaction)
                .await
                .map_err(database_error)?;
        }
    }
    Ok(())
}

async fn soft_delete_department_extras(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    created: &BTreeMap<String, BTreeSet<String>>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let created_paths = created_keys(created, "department").collect::<BTreeSet<_>>();
    let models = dept::Entity::find()
        .filter(dept::Column::TenantId.eq(tenant_id))
        .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
        .all(transaction)
        .await
        .map_err(database_error)?;
    let paths = build_department_paths(&models)?;
    let mut extras = models
        .into_iter()
        .filter_map(|model| {
            paths
                .get(&model.id)
                .filter(|path| {
                    created_paths.contains(&normalize_resource_stable_key(
                        "department",
                        &join_path(path),
                    ))
                })
                .map(|path| (path.len(), model))
        })
        .collect::<Vec<_>>();
    extras.sort_by_key(|item| std::cmp::Reverse(item.0));
    for (_, mut item) in extras {
        item.del_flag = dept::Model::DEL_FLAG_DELETED.to_owned();
        item.updated_at = now;
        dept::ActiveModel::from(item)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}
