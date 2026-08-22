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
