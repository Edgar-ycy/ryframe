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
    capabilities
        .iter()
        .filter(|capability| {
            capability.delegated
                && has_permission(user_permissions, &capability.permission)
                && has_permission(account_permissions, &capability.permission)
        })
        .cloned()
        .collect()
}

fn has_permission(permissions: &HashSet<String>, required: &str) -> bool {
    permissions.iter().any(|permission| {
        ryframe_auth::rbac::has_permission(std::slice::from_ref(permission), required)
    })
}

pub(super) async fn validate_dept(
    transaction: &dyn ServiceAccountWriteTransaction,
    tenant_id: &str,
    dept_id: Option<i64>,
) -> AppResult<()> {
    let Some(dept_id) = dept_id else {
        return Ok(());
    };
    if !transaction.department_exists(tenant_id, dept_id).await? {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ServiceCapabilityDescriptor, common_capabilities};

    fn permissions(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn delegated_capability_requires_both_permission_sets() {
        let capabilities = vec![
            ServiceCapabilityDescriptor {
                key: "read".to_owned(),
                permission: "system:user:list".to_owned(),
                direct: true,
                delegated: true,
            },
            ServiceCapabilityDescriptor {
                key: "write".to_owned(),
                permission: "system:user:create".to_owned(),
                direct: true,
                delegated: false,
            },
        ];

        let common = common_capabilities(
            &capabilities,
            &permissions(&["system:user:*"]),
            &permissions(&["system:user:list"]),
        );

        assert_eq!(common.len(), 1);
        assert_eq!(common[0].key, "read");
    }
}
