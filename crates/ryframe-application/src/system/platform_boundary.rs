/// 平台控制权限不允许进入普通租户的授权或套餐。
pub fn is_platform_permission(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
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
