use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
};

use super::model::{
    AccessCatalog, CATALOG_VERSION, CompiledRoute, ExplicitPolicyKind, GeneratedPolicyKind,
    SUPPORTED_HTTP_METHODS,
};

pub(super) fn validate_catalog(catalog: &AccessCatalog) -> Result<(), Box<dyn Error>> {
    if catalog.version != CATALOG_VERSION {
        return Err(format!(
            "访问目录版本必须为 {CATALOG_VERSION}，实际为 {}",
            catalog.version
        )
        .into());
    }

    let permissions = unique_values("权限码", &catalog.permissions)?;
    if catalog
        .permissions
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err("访问目录 permissions 必须按字典序排列且不得重复".into());
    }
    for permission in &permissions {
        validate_code("权限码", permission, ':')?;
    }
    for (permission, name) in &catalog.permission_names {
        validate_reference("权限名称", permission, "权限码", &permissions)?;
        if name.trim() != name || name.is_empty() || name.chars().count() > 64 {
            return Err(format!("权限 {permission} 的中文名称格式无效").into());
        }
    }

    let non_route_permissions = unique_values("非路由权限码", &catalog.non_route_permissions)?;
    validate_references(
        "非路由权限码",
        &non_route_permissions,
        "权限码",
        &permissions,
    )?;

    let MenuKeys {
        route_keys,
        page_keys,
    } = validate_menus(catalog, &permissions)?;
    validate_capabilities(catalog, &permissions, &route_keys, &page_keys)?;

    let mut explicit_endpoints = BTreeSet::new();
    for policy in &catalog.route_policies {
        validate_method(&policy.method)?;
        validate_api_path(&policy.path)?;
        if !explicit_endpoints.insert((policy.method.as_str(), policy.path.as_str())) {
            return Err(format!("显式路由 policy 重复: {} {}", policy.method, policy.path).into());
        }
    }
    let mut manual_endpoints = BTreeSet::new();
    for route in &catalog.manual_routes {
        validate_method(&route.method)?;
        validate_api_path(&route.path)?;
        if !manual_endpoints.insert((route.method.as_str(), route.path.as_str())) {
            return Err(format!("手工路由重复: {} {}", route.method, route.path).into());
        }
        if explicit_endpoints.contains(&(route.method.as_str(), route.path.as_str())) {
            return Err(format!(
                "手工路由不得重复声明 route_policies: {} {}",
                route.method, route.path
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn append_manual_routes(
    catalog: &AccessCatalog,
    compiled_handlers: &BTreeSet<(String, String)>,
    routes: &mut Vec<CompiledRoute>,
) -> Result<(), Box<dyn Error>> {
    for route in &catalog.manual_routes {
        if !compiled_handlers.contains(&(route.source.clone(), route.handler.clone())) {
            return Err(format!(
                "手工路由处理函数不存在: {}::{}",
                route.source, route.handler
            )
            .into());
        }
        routes.push(CompiledRoute {
            source: route.source.clone(),
            handler: route.handler.clone(),
            method: route.method.clone(),
            path: route.path.clone(),
            permission: None,
            capability: None,
            declared_policy: Some(match route.policy {
                ExplicitPolicyKind::Public => GeneratedPolicyKind::Public,
                ExplicitPolicyKind::Authenticated => GeneratedPolicyKind::Authenticated,
            }),
        });
    }
    Ok(())
}

pub(super) fn validate_routes(
    catalog: &AccessCatalog,
    routes: &[CompiledRoute],
) -> Result<Vec<GeneratedPolicyKind>, Box<dyn Error>> {
    if routes.is_empty() {
        return Err("未发现任何编译期 API 路由".into());
    }
    let permissions = catalog
        .permissions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let non_route_permissions = catalog
        .non_route_permissions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let capabilities = catalog
        .capabilities
        .iter()
        .map(|entry| (entry.code.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let explicit = catalog
        .route_policies
        .iter()
        .map(|entry| ((entry.method.as_str(), entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();

    let mut endpoints = BTreeSet::new();
    let mut used_explicit = BTreeSet::new();
    let mut used_permissions = BTreeSet::new();
    let mut policies = Vec::with_capacity(routes.len());
    let mut missing = Vec::new();
    for route in routes {
        validate_method(&route.method)?;
        validate_api_path(&route.path)?;
        if !endpoints.insert((route.method.as_str(), route.path.as_str())) {
            return Err(format!("编译路由重复: {} {}", route.method, route.path).into());
        }
        if let Some(permission) = route.permission.as_deref() {
            validate_reference("路由权限码", permission, "权限码", &permissions)?;
            used_permissions.insert(permission);
        }
        if let Some(capability) = route.capability.as_deref() {
            let descriptor = capabilities
                .get(capability)
                .ok_or_else(|| format!("路由能力码 {capability} 未在能力目录中声明"))?;
            if let Some(permission) = route.permission.as_deref()
                && !descriptor
                    .permissions
                    .iter()
                    .any(|candidate| candidate == permission)
            {
                return Err(format!(
                    "路由 {} {} 的权限码 {permission} 不属于能力 {capability}",
                    route.method, route.path
                )
                .into());
            }
            policies.push(GeneratedPolicyKind::Capability);
            continue;
        }
        if route.permission.is_some() {
            policies.push(GeneratedPolicyKind::Permission);
            continue;
        }
        if let Some(policy) = route.declared_policy {
            policies.push(policy);
            continue;
        }
        let endpoint = (route.method.as_str(), route.path.as_str());
        let Some(policy) = explicit.get(&endpoint) else {
            missing.push(format!(
                "{} {}（{}::{})",
                route.method, route.path, route.source, route.handler
            ));
            policies.push(GeneratedPolicyKind::Authenticated);
            continue;
        };
        used_explicit.insert(endpoint);
        policies.push(match policy.policy {
            ExplicitPolicyKind::Public => GeneratedPolicyKind::Public,
            ExplicitPolicyKind::Authenticated => GeneratedPolicyKind::Authenticated,
        });
    }
    if !missing.is_empty() {
        return Err(format!(
            "以下路由缺少显式 Public/Authenticated/Permission/Capability policy：\n  - {}",
            missing.join("\n  - ")
        )
        .into());
    }

    let unused_explicit = explicit
        .keys()
        .filter(|endpoint| !used_explicit.contains(*endpoint))
        .map(|(method, path)| format!("{method} {path}"))
        .collect::<Vec<_>>();
    if !unused_explicit.is_empty() {
        return Err(format!(
            "访问目录包含未绑定的显式路由 policy：{}",
            unused_explicit.join(", ")
        )
        .into());
    }

    let unused_route_permissions = permissions
        .difference(&used_permissions)
        .copied()
        .filter(|permission| !non_route_permissions.contains(permission))
        .collect::<Vec<_>>();
    if !unused_route_permissions.is_empty() {
        return Err(format!(
            "访问目录权限码既未绑定路由也未声明为 non_route_permissions：{}",
            unused_route_permissions.join(", ")
        )
        .into());
    }
    let unexpected_non_route = non_route_permissions
        .intersection(&used_permissions)
        .copied()
        .collect::<Vec<_>>();
    if !unexpected_non_route.is_empty() {
        return Err(format!(
            "non_route_permissions 已被路由引用：{}",
            unexpected_non_route.join(", ")
        )
        .into());
    }

    for capability in &catalog.capabilities {
        for permission in &capability.permissions {
            if !routes.iter().any(|route| {
                route.capability.as_deref() == Some(capability.code.as_str())
                    && route.permission.as_deref() == Some(permission.as_str())
            }) {
                return Err(format!(
                    "能力 {} 的权限码 {} 没有对应的编译路由",
                    capability.code, permission
                )
                .into());
            }
        }
    }
    Ok(policies)
}

pub(super) fn validate_code(
    label: &str,
    value: &str,
    separator: char,
) -> Result<(), Box<dyn Error>> {
    if value.trim() != value
        || value.is_empty()
        || !value.contains(separator)
        || value.chars().any(char::is_whitespace)
    {
        return Err(format!("{label}格式无效: {value:?}").into());
    }
    Ok(())
}

pub(super) fn validate_identifier(label: &str, value: &str) -> Result<(), Box<dyn Error>> {
    if value.trim() != value
        || value.is_empty()
        || value.chars().any(|character| {
            !character.is_ascii_alphanumeric()
                && character != '.'
                && character != '-'
                && character != '_'
        })
    {
        return Err(format!("{label}格式无效: {value:?}").into());
    }
    Ok(())
}

fn unique_values<'a>(
    label: &str,
    values: &'a [String],
) -> Result<BTreeSet<&'a str>, Box<dyn Error>> {
    let result = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if result.len() != values.len() {
        return Err(format!("{label}包含重复项").into());
    }
    Ok(result)
}

fn validate_reference(
    label: &str,
    value: &str,
    target_label: &str,
    targets: &BTreeSet<&str>,
) -> Result<(), Box<dyn Error>> {
    if !targets.contains(value) {
        return Err(format!("{label} {value} 未在{target_label}目录中声明").into());
    }
    Ok(())
}

fn validate_references(
    label: &str,
    values: &BTreeSet<&str>,
    target_label: &str,
    targets: &BTreeSet<&str>,
) -> Result<(), Box<dyn Error>> {
    for value in values {
        validate_reference(label, value, target_label, targets)?;
    }
    Ok(())
}

fn validate_method(method: &str) -> Result<(), Box<dyn Error>> {
    if SUPPORTED_HTTP_METHODS.contains(&method) {
        Ok(())
    } else {
        Err(format!("不支持的 HTTP 方法: {method}").into())
    }
}

fn validate_api_path(path: &str) -> Result<(), Box<dyn Error>> {
    if path.starts_with('/') && !path.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(format!("HTTP 路径必须是无空白的绝对路径: {path}").into())
    }
}

struct MenuKeys<'a> {
    route_keys: BTreeSet<&'a str>,
    page_keys: BTreeSet<&'a str>,
}

fn validate_menus<'a>(
    catalog: &'a AccessCatalog,
    permissions: &BTreeSet<&str>,
) -> Result<MenuKeys<'a>, Box<dyn Error>> {
    let mut route_keys = BTreeSet::new();
    let mut page_keys = BTreeSet::new();
    for menu in &catalog.menus {
        validate_identifier("菜单 route_key", &menu.route_key)?;
        if menu.order > i32::MAX as u32 {
            return Err(format!("菜单 {} 的 order 超出可表示范围", menu.route_key).into());
        }
        validate_identifier("菜单 title_key", &menu.title_key)?;
        if menu.name.trim() != menu.name || menu.name.is_empty() || menu.name.chars().count() > 64 {
            return Err(format!("菜单 {} 的 name 格式无效", menu.route_key).into());
        }
        if !route_keys.insert(menu.route_key.as_str()) {
            return Err(format!("菜单 route_key 重复: {}", menu.route_key).into());
        }
        if menu.menu_type != "M" && menu.menu_type != "C" {
            return Err(format!("菜单 {} 的 menu_type 只能是 M 或 C", menu.route_key).into());
        }
        match (&*menu.menu_type, menu.page_key.as_deref()) {
            ("M", None) => {}
            ("M", Some(_)) => {
                return Err(format!("目录菜单 {} 不得声明 page_key", menu.route_key).into());
            }
            ("C", Some(page_key)) => {
                validate_identifier("页面 page_key", page_key)?;
                if !page_keys.insert(page_key) {
                    return Err(format!("页面 page_key 重复: {page_key}").into());
                }
            }
            ("C", None) => {
                return Err(format!("页面菜单 {} 必须声明 page_key", menu.route_key).into());
            }
            _ => unreachable!("menu_type 已校验"),
        }
        if let Some(permission) = menu.permission.as_deref() {
            validate_reference("菜单权限码", permission, "权限码", permissions)?;
        }
    }

    Ok(MenuKeys {
        route_keys,
        page_keys,
    })
}

fn validate_capabilities(
    catalog: &AccessCatalog,
    permissions: &BTreeSet<&str>,
    route_keys: &BTreeSet<&str>,
    page_keys: &BTreeSet<&str>,
) -> Result<(), Box<dyn Error>> {
    let mut capabilities = BTreeSet::new();
    for capability in &catalog.capabilities {
        validate_code("能力码", &capability.code, '.')?;
        if !capabilities.insert(capability.code.as_str()) {
            return Err(format!("能力码重复: {}", capability.code).into());
        }
        let capability_route_keys = unique_values("能力 route_key", &capability.route_keys)?;
        validate_references(
            "能力 route_key",
            &capability_route_keys,
            "菜单 route_key",
            route_keys,
        )?;
        for route_key in &capability_route_keys {
            let menu = catalog
                .menus
                .iter()
                .find(|menu| menu.route_key == *route_key)
                .expect("能力 route_key 已通过引用校验");
            if menu.capability.as_deref() != Some(capability.code.as_str()) {
                return Err(format!(
                    "能力 {} 的 route_key {} 未反向绑定同一菜单能力",
                    capability.code, route_key
                )
                .into());
            }
        }
        let capability_page_keys = unique_values("能力 page_key", &capability.page_keys)?;
        validate_references(
            "能力 page_key",
            &capability_page_keys,
            "页面 page_key",
            page_keys,
        )?;
        let capability_permissions = unique_values("能力权限码", &capability.permissions)?;
        validate_references("能力权限码", &capability_permissions, "权限码", permissions)?;
    }
    for menu in &catalog.menus {
        if let Some(capability) = menu.capability.as_deref() {
            validate_reference("菜单能力码", capability, "能力码", &capabilities)?;
            let descriptor = catalog
                .capabilities
                .iter()
                .find(|descriptor| descriptor.code == capability)
                .expect("菜单能力码已通过引用校验");
            if !descriptor.route_keys.contains(&menu.route_key) {
                return Err(format!(
                    "菜单 {} 未闭合到能力 {} 的 route_keys",
                    menu.route_key, capability
                )
                .into());
            }
            if let Some(page_key) = menu.page_key.as_ref()
                && !descriptor.page_keys.contains(page_key)
            {
                return Err(
                    format!("页面 {page_key} 未闭合到能力 {capability} 的 page_keys").into(),
                );
            }
            if let Some(permission) = menu.permission.as_ref()
                && !descriptor.permissions.contains(permission)
            {
                return Err(format!(
                    "菜单权限 {permission} 未闭合到能力 {capability} 的 permissions"
                )
                .into());
            }
        }
    }

    Ok(())
}
