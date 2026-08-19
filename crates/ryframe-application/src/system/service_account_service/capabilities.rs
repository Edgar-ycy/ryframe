use super::*;

pub(super) fn validate_capabilities(capabilities: &[ServiceCapabilityDescriptor]) -> AppResult<()> {
    let mut keys = BTreeSet::new();
    for capability in capabilities {
        if capability.key.trim().is_empty() || capability.permission.trim().is_empty() {
            return Err(AppError::Config(
                "服务能力 key 和 permission 不能为空".into(),
            ));
        }
        if !capability.direct && !capability.delegated {
            return Err(AppError::Config(format!(
                "服务能力 {} 未启用任何访问模式",
                capability.key
            )));
        }
        if !keys.insert(capability.key.as_str()) {
            return Err(AppError::Config(format!(
                "服务能力 {} 重复",
                capability.key
            )));
        }
    }
    Ok(())
}

pub(super) fn common_capabilities(
    capabilities: &[ServiceCapabilityDescriptor],
    user_permissions: &HashSet<String>,
    account_permissions: &HashSet<String>,
) -> Vec<ServiceCapabilityDescriptor> {
    let user_permissions = user_permissions.iter().cloned().collect::<Vec<_>>();
    let account_permissions = account_permissions.iter().cloned().collect::<Vec<_>>();
    capabilities
        .iter()
        .filter(|capability| {
            capability.delegated
                && ryframe_auth::rbac::has_permission(
                    &user_permissions,
                    capability.permission.as_str(),
                )
                && ryframe_auth::rbac::has_permission(
                    &account_permissions,
                    capability.permission.as_str(),
                )
        })
        .cloned()
        .collect()
}

pub(super) async fn account_permission_codes_for_accounts(
    db: &DatabaseConnection,
    tenant_id: &str,
    account_ids: &[i64],
) -> AppResult<HashMap<i64, HashSet<String>>> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let account_roles = service_account_role::Entity::find()
        .filter(service_account_role::Column::TenantId.eq(tenant_id))
        .filter(service_account_role::Column::AccountId.is_in(account_ids.iter().copied()))
        .all(db)
        .await
        .map_err(database_error)?;
    if account_roles.is_empty() {
        return Ok(HashMap::new());
    }
    let role_ids = account_roles
        .iter()
        .map(|relation| relation.role_id)
        .collect::<HashSet<_>>();
    let enabled_role_ids = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::Id.is_in(role_ids.iter().copied()))
        .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|role| role.id)
        .collect::<HashSet<_>>();
    if enabled_role_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let role_permissions = ryframe_db::entities::role_permission::Entity::find()
        .filter(ryframe_db::entities::role_permission::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::role_permission::Column::RoleId
                .is_in(enabled_role_ids.iter().copied()),
        )
        .all(db)
        .await
        .map_err(database_error)?;
    let permission_ids = role_permissions
        .iter()
        .map(|relation| relation.perm_id)
        .collect::<HashSet<_>>();
    let permission_codes = if permission_ids.is_empty() {
        HashMap::new()
    } else {
        ryframe_db::entities::permission::Entity::find()
            .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
            .filter(
                ryframe_db::entities::permission::Column::Id.is_in(permission_ids.iter().copied()),
            )
            .filter(ryframe_db::entities::permission::Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(|permission| (permission.id, permission.code))
            .collect::<HashMap<_, _>>()
    };
    let role_to_permissions = role_permissions.into_iter().fold(
        HashMap::<i64, HashSet<String>>::new(),
        |mut mapping, relation| {
            if let Some(code) = permission_codes.get(&relation.perm_id) {
                mapping
                    .entry(relation.role_id)
                    .or_default()
                    .insert(code.clone());
            }
            mapping
        },
    );
    let mut result = HashMap::<i64, HashSet<String>>::new();
    for relation in account_roles {
        if !enabled_role_ids.contains(&relation.role_id) {
            continue;
        }
        if let Some(codes) = role_to_permissions.get(&relation.role_id) {
            result
                .entry(relation.account_id)
                .or_default()
                .extend(codes.iter().cloned());
        }
    }
    Ok(result)
}

pub(super) async fn validate_dept(
    db: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    dept_id: Option<i64>,
) -> AppResult<()> {
    let Some(dept_id) = dept_id else {
        return Ok(());
    };
    if ryframe_db::entities::dept::Entity::find_by_id(dept_id)
        .filter(ryframe_db::entities::dept::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::dept::Column::DelFlag
                .eq(ryframe_db::entities::dept::Model::DEL_FLAG_NORMAL),
        )
        .lock(LockType::Share)
        .one(db)
        .await
        .map_err(database_error)?
        .is_none()
    {
        return Err(AppError::Validation("部门不存在或不属于当前租户".into()));
    }
    Ok(())
}

pub(super) async fn permission_codes_in_txn(
    db: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    role_ids: &[i64],
) -> AppResult<HashSet<String>> {
    if role_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let permission_ids = ryframe_db::entities::role_permission::Entity::find()
        .filter(ryframe_db::entities::role_permission::Column::TenantId.eq(tenant_id))
        .filter(
            ryframe_db::entities::role_permission::Column::RoleId.is_in(role_ids.iter().copied()),
        )
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| row.perm_id)
        .collect::<Vec<_>>();
    if permission_ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(ryframe_db::entities::permission::Entity::find()
        .filter(ryframe_db::entities::permission::Column::TenantId.eq(tenant_id))
        .filter(ryframe_db::entities::permission::Column::Id.is_in(permission_ids))
        .filter(ryframe_db::entities::permission::Column::Status.eq("1"))
        .all(db)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|permission| permission.code)
        .collect())
}

pub(super) fn delegation_vo_with_keys(
    delegation: service_delegation::Model,
    capability_keys: Vec<String>,
) -> ServiceDelegationVo {
    ServiceDelegationVo {
        id: delegation.id.to_string(),
        account_id: delegation.account_id.to_string(),
        user_id: delegation.user_id.to_string(),
        status: delegation.status,
        version: delegation.version,
        not_before: delegation.not_before,
        expires_at: delegation.expires_at,
        reason: delegation.reason,
        capability_keys,
        revoked_at: delegation.revoked_at,
        created_at: delegation.created_at,
    }
}
