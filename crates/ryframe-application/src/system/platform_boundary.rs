use std::{collections::BTreeSet, sync::OnceLock};

use ryframe_kernel::{AppError, AppResult};

#[derive(Debug, Eq, PartialEq)]
struct BusinessSystemAccess {
    all_permissions: BTreeSet<String>,
    default_admin_permissions: BTreeSet<String>,
    permissions: BTreeSet<String>,
    routes: BTreeSet<String>,
}

static BUSINESS_SYSTEM_ACCESS: OnceLock<BusinessSystemAccess> = OnceLock::new();

/// 组合根从已校验的业务访问目录安装系统租户边界；运行期间不允许替换。
pub fn register_business_access_scope(
    all_permissions: impl IntoIterator<Item = String>,
    default_admin_permissions: impl IntoIterator<Item = String>,
    permissions: impl IntoIterator<Item = String>,
    routes: impl IntoIterator<Item = String>,
) -> AppResult<()> {
    let candidate = BusinessSystemAccess {
        all_permissions: all_permissions.into_iter().collect(),
        default_admin_permissions: default_admin_permissions.into_iter().collect(),
        permissions: permissions.into_iter().collect(),
        routes: routes.into_iter().collect(),
    };
    if !candidate.permissions.is_subset(&candidate.all_permissions)
        || !candidate
            .default_admin_permissions
            .is_subset(&candidate.all_permissions)
        || !candidate
            .default_admin_permissions
            .is_disjoint(&candidate.permissions)
    {
        return Err(AppError::Config(
            "业务默认授权必须来自租户业务目录，系统租户专用权限不得进入租户默认授权".into(),
        ));
    }
    for value in candidate.all_permissions.iter().chain(&candidate.routes) {
        if value.is_empty()
            || value != &value.to_ascii_lowercase()
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b":.-".contains(&byte)
            })
        {
            return Err(AppError::Config("业务系统租户访问目录包含非法标识".into()));
        }
    }
    if let Some(current) = BUSINESS_SYSTEM_ACCESS.get() {
        return same_business_access(current, &candidate);
    }
    match BUSINESS_SYSTEM_ACCESS.set(candidate) {
        Ok(()) => Ok(()),
        Err(candidate) => same_business_access(
            BUSINESS_SYSTEM_ACCESS
                .get()
                .ok_or_else(|| AppError::Config("业务访问边界安装后不可用".into()))?,
            &candidate,
        ),
    }
}

pub fn is_business_permission(code: &str) -> bool {
    BUSINESS_SYSTEM_ACCESS
        .get()
        .is_some_and(|access| access.all_permissions.contains(&code.to_ascii_lowercase()))
}

pub(super) fn is_business_default_admin_permission(code: &str) -> bool {
    BUSINESS_SYSTEM_ACCESS.get().is_some_and(|access| {
        access
            .default_admin_permissions
            .contains(&code.to_ascii_lowercase())
    })
}

fn same_business_access(
    current: &BusinessSystemAccess,
    candidate: &BusinessSystemAccess,
) -> AppResult<()> {
    if current == candidate {
        Ok(())
    } else {
        Err(AppError::Config(
            "业务系统租户访问边界已安装，不能在运行期间替换".into(),
        ))
    }
}

/// 平台控制和全局业务权限不允许进入普通租户的授权或套餐。
pub fn is_platform_permission(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    if BUSINESS_SYSTEM_ACCESS
        .get()
        .is_some_and(|access| access.permissions.contains(&code))
    {
        return true;
    }
    [
        "tenant:",
        "platform:",
        "monitor:retention:",
        "monitor:server:",
        "monitor:cache:",
        "monitor:db-pool:",
        "monitor:runtime:",
        "monitor:overview:",
    ]
    .iter()
    .any(|prefix| code.starts_with(prefix))
}

pub fn is_platform_route(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    let key = normalized.as_str();
    if BUSINESS_SYSTEM_ACCESS
        .get()
        .is_some_and(|access| access.routes.contains(key))
    {
        return true;
    }
    key == "platform"
        || key.starts_with("platform.")
        || matches!(
            key,
            "monitor.server"
                | "monitor.cache"
                | "monitor.db-pool"
                | "monitor.runtime"
                | "monitor.overview"
                | "monitor.retention"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_management_and_infrastructure_are_platform_only() {
        for code in [
            "tenant:add",
            "platform:product-plan:list",
            "monitor:cache:list",
            "monitor:server:list",
            "monitor:db-pool:list",
            "platform:config-transfer:apply",
        ] {
            assert!(is_platform_permission(code));
        }
        for code in [
            "system:user:add",
            "system:menu:list",
            "monitor:schedule:list",
            "monitor:job:retry",
        ] {
            assert!(!is_platform_permission(code));
        }
        assert!(is_platform_route("platform.tenant"));
        assert!(!is_platform_route("system.user"));
    }
}
