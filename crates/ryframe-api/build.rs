use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use syn::{Attribute, Expr, ExprLit, Item, Lit, LitStr, Meta, Token, punctuated::Punctuated};

const CATALOG_VERSION: u32 = 1;
const SUPPORTED_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessCatalog {
    version: u32,
    permissions: Vec<String>,
    #[serde(default)]
    non_route_permissions: Vec<String>,
    #[serde(default)]
    permission_names: BTreeMap<String, String>,
    #[serde(default)]
    menus: Vec<MenuEntry>,
    #[serde(default)]
    capabilities: Vec<CapabilityEntry>,
    #[serde(default)]
    route_policies: Vec<ExplicitRoutePolicy>,
    #[serde(default)]
    manual_routes: Vec<ManualRoute>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MenuEntry {
    route_key: String,
    name: String,
    title_key: String,
    menu_type: String,
    page_key: Option<String>,
    permission: Option<String>,
    capability: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityEntry {
    code: String,
    route_keys: Vec<String>,
    page_keys: Vec<String>,
    permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplicitRoutePolicy {
    method: String,
    path: String,
    policy: ExplicitPolicyKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualRoute {
    source: String,
    handler: String,
    method: String,
    path: String,
    policy: ExplicitPolicyKind,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExplicitPolicyKind {
    Public,
    Authenticated,
}

#[derive(Clone, Copy, Debug)]
enum GeneratedPolicyKind {
    Public,
    Authenticated,
    Permission,
    Capability,
}

impl GeneratedPolicyKind {
    const fn rust_name(self) -> &'static str {
        match self {
            Self::Public => "Public",
            Self::Authenticated => "Authenticated",
            Self::Permission => "Permission",
            Self::Capability => "Capability",
        }
    }
}

#[derive(Debug)]
struct CompiledRoute {
    source: String,
    handler: String,
    method: String,
    path: String,
    permission: Option<String>,
    capability: Option<String>,
    declared_policy: Option<GeneratedPolicyKind>,
}

fn main() -> Result<(), Box<dyn Error>> {
    configure_build_commit()?;

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("ryframe-api 必须位于工作区 crates 目录下")?;
    let catalog_path = workspace_root.join("catalog").join("access.toml");
    println!("cargo:rerun-if-changed={}", catalog_path.display());
    let catalog_source = fs::read_to_string(&catalog_path)?;
    let catalog: AccessCatalog = toml::from_str(&catalog_source)
        .map_err(|error| format!("{} 不是有效的访问目录: {error}", catalog_path.display()))?;
    validate_catalog(&catalog)?;

    let source_root = manifest_dir.join("src");
    println!("cargo:rerun-if-changed={}", source_root.display());
    let mut source_files = Vec::new();
    collect_rust_files(&source_root, &mut source_files)?;
    source_files.sort();

    let mut routes = Vec::new();
    let mut compiled_handlers = BTreeSet::new();
    for path in source_files {
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(&path)?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("无法解析 {}: {error}", path.display()))?;
        let source_label = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        collect_routes(
            &file.items,
            &mut routes,
            &mut compiled_handlers,
            &source_label,
            "",
        )?;
    }

    append_manual_routes(&catalog, &compiled_handlers, &mut routes)?;
    routes.sort_by(|left, right| {
        (
            &left.source,
            &left.handler,
            &left.method,
            &left.path,
            &left.capability,
            &left.permission,
        )
            .cmp(&(
                &right.source,
                &right.handler,
                &right.method,
                &right.path,
                &right.capability,
                &right.permission,
            ))
    });

    let policies = validate_routes(&catalog, &routes)?;
    let generated = render_catalog(&catalog, &routes, &policies);
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("permission_catalog.rs"),
        generated,
    )?;
    Ok(())
}

fn configure_build_commit() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=RYFRAME_BUILD_COMMIT");
    let build_commit = env::var("RYFRAME_BUILD_COMMIT")
        .unwrap_or_else(|_| "development".to_owned())
        .trim()
        .to_ascii_lowercase();
    if build_commit != "development"
        && (build_commit.len() != 40 || !build_commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("RYFRAME_BUILD_COMMIT 必须是完整的 40 位 Git 提交 SHA".into());
    }
    println!("cargo:rustc-env=RYFRAME_BUILD_COMMIT={build_commit}");
    Ok(())
}

fn validate_catalog(catalog: &AccessCatalog) -> Result<(), Box<dyn Error>> {
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

    let mut route_keys = BTreeSet::new();
    let mut page_keys = BTreeSet::new();
    for menu in &catalog.menus {
        validate_identifier("菜单 route_key", &menu.route_key)?;
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
            validate_reference("菜单权限码", permission, "权限码", &permissions)?;
        }
    }

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
            &route_keys,
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
            &page_keys,
        )?;
        let capability_permissions = unique_values("能力权限码", &capability.permissions)?;
        validate_references(
            "能力权限码",
            &capability_permissions,
            "权限码",
            &permissions,
        )?;
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

fn validate_code(label: &str, value: &str, separator: char) -> Result<(), Box<dyn Error>> {
    if value.trim() != value
        || value.is_empty()
        || !value.contains(separator)
        || value.chars().any(char::is_whitespace)
    {
        return Err(format!("{label}格式无效: {value:?}").into());
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), Box<dyn Error>> {
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

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_routes(
    items: &[Item],
    routes: &mut Vec<CompiledRoute>,
    compiled_handlers: &mut BTreeSet<(String, String)>,
    source: &str,
    module_path: &str,
) -> syn::Result<()> {
    for item in items {
        match item {
            Item::Fn(function) => {
                let handler = qualified_handler(module_path, &function.sig.ident.to_string());
                compiled_handlers.insert((source.to_owned(), handler.clone()));
                let (permission, capability) = collect_access_attributes(&function.attrs)?;
                let route_method = route_attribute_method(&function.attrs)?;
                let documented = documented_route(&function.attrs)?;
                if route_method.is_some() || permission.is_some() || capability.is_some() {
                    let Some((documented_method, documented_path)) = documented else {
                        return Err(syn::Error::new_spanned(
                            function,
                            "HTTP 路由必须声明唯一的 #[utoipa::path(...)] 契约",
                        ));
                    };
                    if let Some(route_method) = route_method
                        && route_method != documented_method
                    {
                        return Err(syn::Error::new_spanned(
                            function,
                            format!(
                                "路由属性方法 {route_method} 与 utoipa 方法 {documented_method} 不一致"
                            ),
                        ));
                    }
                    routes.push(CompiledRoute {
                        source: source.to_owned(),
                        handler,
                        method: documented_method,
                        path: documented_path,
                        permission,
                        capability,
                        declared_policy: None,
                    });
                } else if let Some((method, path)) = documented {
                    routes.push(CompiledRoute {
                        source: source.to_owned(),
                        handler,
                        method,
                        path,
                        permission: None,
                        capability: None,
                        declared_policy: None,
                    });
                }
            }
            Item::Mod(module) => {
                let _ = collect_access_attributes(&module.attrs)?;
                if let Some((_, nested_items)) = &module.content {
                    let nested_path = qualified_handler(module_path, &module.ident.to_string());
                    collect_routes(
                        nested_items,
                        routes,
                        compiled_handlers,
                        source,
                        &nested_path,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn append_manual_routes(
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

fn qualified_handler(module_path: &str, name: &str) -> String {
    if module_path.is_empty() {
        name.to_owned()
    } else {
        format!("{module_path}::{name}")
    }
}

fn collect_access_attributes(
    attributes: &[Attribute],
) -> syn::Result<(Option<String>, Option<String>)> {
    let mut permission = None;
    let mut capability = None;
    for attribute in attributes {
        let marker = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        if marker.as_deref() != Some("perm") && marker.as_deref() != Some("capability") {
            continue;
        }
        let literal = attribute.parse_args::<LitStr>()?;
        let value = literal.value();
        if value.trim() != value || value.is_empty() {
            return Err(syn::Error::new_spanned(
                attribute,
                "访问标记不得为空或包含首尾空白",
            ));
        }
        let slot = if marker.as_deref() == Some("perm") {
            &mut permission
        } else {
            &mut capability
        };
        if slot.replace(value).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个路由最多声明一个同类访问标记",
            ));
        }
    }
    Ok((permission, capability))
}

fn route_attribute_method(attributes: &[Attribute]) -> syn::Result<Option<String>> {
    let mut method = None;
    for attribute in attributes {
        let Some(candidate) = attribute
            .path()
            .segments
            .last()
            .map(|segment| segment.ident.to_string().to_ascii_uppercase())
            .filter(|candidate| SUPPORTED_HTTP_METHODS.contains(&candidate.as_str()))
        else {
            continue;
        };
        let paths = attribute.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
        if paths.len() != 1 {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数必须声明且只能声明一个 HTTP 路径",
            ));
        }
        let path = paths.first().expect("已校验路由路径数量").value();
        if !path.starts_with('/') || path.chars().any(char::is_whitespace) {
            return Err(syn::Error::new_spanned(
                attribute,
                "HTTP 路由属性必须使用无空白的绝对路径",
            ));
        }
        if method.replace(candidate).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数只能声明一个 HTTP 路由属性",
            ));
        }
    }
    Ok(method)
}

fn documented_route(attributes: &[Attribute]) -> syn::Result<Option<(String, String)>> {
    let mut documented = None;
    for attribute in attributes {
        let segments = &attribute.path().segments;
        if segments.len() != 2
            || segments
                .first()
                .is_none_or(|segment| segment.ident != "utoipa")
            || segments
                .last()
                .is_none_or(|segment| segment.ident != "path")
        {
            continue;
        }
        let entries = attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        let mut method = None;
        let mut path = None;
        for entry in entries {
            match entry {
                Meta::Path(candidate) => {
                    let Some(candidate) = candidate
                        .get_ident()
                        .map(|ident| ident.to_string().to_ascii_uppercase())
                        .filter(|candidate| SUPPORTED_HTTP_METHODS.contains(&candidate.as_str()))
                    else {
                        continue;
                    };
                    if method.replace(candidate).is_some() {
                        return Err(syn::Error::new_spanned(
                            attribute,
                            "utoipa 路由只能声明一个 HTTP 方法",
                        ));
                    }
                }
                Meta::NameValue(entry) if entry.path.is_ident("path") => {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Str(value),
                        ..
                    }) = &entry.value
                    else {
                        return Err(syn::Error::new_spanned(
                            entry.value,
                            "utoipa 路径必须是字符串字面量",
                        ));
                    };
                    if path.replace(value.value()).is_some() {
                        return Err(syn::Error::new_spanned(
                            value,
                            "utoipa 路由只能声明一个 path",
                        ));
                    }
                }
                _ => {}
            }
        }
        let method = method.ok_or_else(|| {
            syn::Error::new_spanned(attribute, "utoipa 路由必须声明受支持的 HTTP 方法")
        })?;
        let path =
            path.ok_or_else(|| syn::Error::new_spanned(attribute, "utoipa 路由必须声明显式 path"))?;
        if documented.replace((method, path)).is_some() {
            return Err(syn::Error::new_spanned(
                attribute,
                "每个处理函数只能声明一个 utoipa 路由",
            ));
        }
    }
    Ok(documented)
}

fn validate_routes(
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

fn render_catalog(
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
