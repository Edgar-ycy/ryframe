/// HTTP 路由的编译期访问策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPolicy {
    Public,
    Authenticated,
    Permission,
    Capability,
}

/// 菜单、页面与访问约束的稳定映射。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuRouteDescriptor {
    pub route_key: &'static str,
    pub name: &'static str,
    pub title_key: &'static str,
    pub menu_type: &'static str,
    pub page_key: Option<&'static str>,
    pub permission_code: Option<&'static str>,
    pub capability_code: Option<&'static str>,
}

/// 产品能力对菜单、页面和权限的闭合集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityDescriptor {
    pub code: &'static str,
    pub route_keys: &'static [&'static str],
    pub page_keys: &'static [&'static str],
    pub permission_codes: &'static [&'static str],
}

/// 每个编译路由对应的显式访问策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutePolicyDescriptor {
    pub source: &'static str,
    pub handler: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub policy: AccessPolicy,
    pub permission_code: Option<&'static str>,
    pub capability_code: Option<&'static str>,
}

/// 能力门禁路由的 OpenAPI 契约信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteCapabilityBinding {
    pub source: &'static str,
    pub handler: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub capability_code: &'static str,
    pub permission_code: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"));

pub const SUPPORTED_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];

/// 返回机器目录声明的全部权限码，包括不直接绑定路由的治理权限。
pub fn permission_codes() -> &'static [&'static str] {
    PERMISSION_CODES
}

pub fn menu_routes() -> &'static [MenuRouteDescriptor] {
    MENU_ROUTES
}

pub fn capabilities() -> &'static [CapabilityDescriptor] {
    CAPABILITIES
}

pub fn route_policies() -> &'static [RoutePolicyDescriptor] {
    ROUTE_POLICIES
}

pub fn route_capability_bindings() -> &'static [RouteCapabilityBinding] {
    ROUTE_CAPABILITY_BINDINGS
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn catalog_contains_governance_permission_and_unique_sets() {
        assert!(permission_codes().contains(&"tenant:capability:override"));
        assert_eq!(
            permission_codes().len(),
            permission_codes()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            menu_routes().len(),
            menu_routes()
                .iter()
                .map(|menu| menu.route_key)
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            route_policies().len(),
            route_policies()
                .iter()
                .map(|route| (route.method, route.path))
                .collect::<BTreeSet<_>>()
                .len()
        );
    }

    #[test]
    fn generated_menu_page_keys_are_typed_and_unique() {
        let mut page_keys = BTreeSet::new();
        for menu in menu_routes() {
            match menu.menu_type {
                "M" => assert_eq!(
                    menu.page_key, None,
                    "目录菜单 {} 不得声明 page_key",
                    menu.route_key
                ),
                "C" => {
                    let page_key = menu
                        .page_key
                        .filter(|page_key| !page_key.is_empty())
                        .expect("页面菜单必须生成非空 page_key");
                    assert!(
                        page_keys.insert(page_key),
                        "页面 page_key 必须唯一: {page_key}"
                    );
                }
                menu_type => panic!("访问目录存在未知菜单类型: {menu_type}"),
            }
        }
    }

    #[test]
    fn route_policy_constraints_match_policy_kind() {
        assert!(SUPPORTED_HTTP_METHODS.contains(&"PATCH"));
        for route in route_policies() {
            assert!(SUPPORTED_HTTP_METHODS.contains(&route.method));
            match route.policy {
                AccessPolicy::Public | AccessPolicy::Authenticated => {
                    assert_eq!(route.permission_code, None);
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Permission => {
                    assert!(route.permission_code.is_some());
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Capability => assert!(route.capability_code.is_some()),
            }
        }
    }

    #[test]
    fn menu_references_are_closed() {
        let permissions = permission_codes().iter().copied().collect::<BTreeSet<_>>();
        let capabilities = capabilities()
            .iter()
            .map(|capability| capability.code)
            .collect::<BTreeSet<_>>();
        for menu in menu_routes() {
            if let Some(permission) = menu.permission_code {
                assert!(permissions.contains(permission));
            }
            if let Some(capability) = menu.capability_code {
                assert!(capabilities.contains(capability));
            }
        }
    }

    #[test]
    fn capability_catalog_matches_application_contract() {
        assert_eq!(
            capabilities().len(),
            ryframe_application::system::CAPABILITY_CATALOG.len()
        );
        for capability in capabilities() {
            let application = ryframe_application::system::CAPABILITY_CATALOG
                .iter()
                .find(|candidate| candidate.code == capability.code)
                .expect("访问目录能力必须存在于应用能力目录");
            assert_eq!(capability.route_keys, application.route_keys);
            assert_eq!(capability.permission_codes, application.permission_codes);
        }
    }
}
