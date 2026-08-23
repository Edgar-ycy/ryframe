use super::model::{AccessCatalog, CompiledRoute, GeneratedPolicyKind};

pub(super) fn render_catalog(
    catalog: &AccessCatalog,
    routes: &[CompiledRoute],
    policies: &[GeneratedPolicyKind],
) -> String {
    let mut generated = String::new();
    generated.push_str("const PERMISSION_CODES: &[&str] = &[\n");
    for permission in &catalog.permissions {
        generated.push_str(&format!("    {permission:?},\n"));
    }
    generated.push_str("];\n");

    generated.push_str("const MENU_ROUTES: &[MenuRouteDescriptor] = &[\n");
    for menu in &catalog.menus {
        generated.push_str(&format!(
            "    MenuRouteDescriptor {{ route_key: {:?}, name: {:?}, title_key: {:?}, menu_type: {:?}, page_key: {:?}, permission_code: {:?}, capability_code: {:?} }},\n",
            menu.route_key,
            menu.name,
            menu.title_key,
            menu.menu_type,
            menu.page_key,
            menu.permission,
            menu.capability
        ));
    }
    generated.push_str("];\n");

    generated.push_str("const CAPABILITIES: &[CapabilityDescriptor] = &[\n");
    for capability in &catalog.capabilities {
        generated.push_str(&format!(
            "    CapabilityDescriptor {{ code: {:?}, route_keys: &{:?}, page_keys: &{:?}, permission_codes: &{:?} }},\n",
            capability.code,
            capability.route_keys,
            capability.page_keys,
            capability.permissions
        ));
    }
    generated.push_str("];\n");

    generated.push_str("const ROUTE_POLICIES: &[RoutePolicyDescriptor] = &[\n");
    for (route, policy) in routes.iter().zip(policies) {
        generated.push_str(&format!(
            "    RoutePolicyDescriptor {{ source: {:?}, handler: {:?}, method: {:?}, path: {:?}, policy: AccessPolicy::{}, permission_code: {:?}, capability_code: {:?} }},\n",
            route.source,
            route.handler,
            route.method,
            route.path,
            policy.rust_name(),
            route.permission,
            route.capability
        ));
    }
    generated.push_str("];\n");

    generated.push_str("const ROUTE_CAPABILITY_BINDINGS: &[RouteCapabilityBinding] = &[\n");
    for route in routes.iter().filter(|route| route.capability.is_some()) {
        generated.push_str(&format!(
            "    RouteCapabilityBinding {{ source: {:?}, handler: {:?}, method: {:?}, path: {:?}, capability_code: {:?}, permission_code: {:?} }},\n",
            route.source,
            route.handler,
            route.method,
            route.path,
            route.capability.as_deref().expect("已筛选能力路由"),
            route.permission
        ));
    }
    generated.push_str("];\n");
    generated
}
