use std::collections::{BTreeMap, BTreeSet};

use super::{
    ResourceError, ResourceSpec, is_operation_symbol, is_permission, is_safe_route, is_safe_symbol,
    is_snake_identifier, operations, permissions, validate_labels,
};

pub(super) fn validate_api_and_access(
    resource: &str,
    source_path: &str,
    spec: &ResourceSpec,
) -> Result<(), ResourceError> {
    for (label, path) in [("API", &spec.api.path), ("路由", &spec.route.path)] {
        if !is_safe_route(path) {
            return Err(ResourceError::new(
                format!("{label}路径 `{path}` 无效"),
                "路径必须以 / 开头，且不能包含空白、查询串或父目录片段",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
    }
    let operations = operations(spec.api.operations.clone());
    let operation_values = operations.values();
    if operation_values
        .iter()
        .any(|value| !is_operation_symbol(value))
    {
        return Err(ResourceError::new(
            "operationId 必须是安全标识符",
            "使用 OpenAPI 中的精确 operationId，例如 `get_system_devices`",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    if operation_values.into_iter().collect::<BTreeSet<_>>().len() != 5 {
        return Err(ResourceError::new(
            "operationId 不能重复",
            "为 create/read/list/update/delete 分配唯一 operationId",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    let permission_values = permissions(spec.access.permissions.clone());
    if permission_values
        .values()
        .iter()
        .any(|value| !is_permission(value))
    {
        return Err(ResourceError::new(
            "权限标识格式无效",
            "使用 `module:resource-name:action-name` 形式的三段 kebab-case 权限标识",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    if let Some(capability) = &spec.access.capability
        && !is_safe_symbol(capability)
    {
        return Err(ResourceError::new(
            format!("capability `{capability}` 格式无效"),
            "只使用字母、数字、下划线、短横线或点",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    for (label, value) in [
        ("menu.key", &spec.menu.key),
        ("menu.parent", &spec.menu.parent),
        ("route.key", &spec.route.key),
    ] {
        if !is_safe_symbol(value) {
            return Err(ResourceError::new(
                format!("{label} `{value}` 格式无效"),
                "只使用字母、数字、下划线、短横线或点",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
    }
    if spec.route.key != spec.menu.key {
        return Err(ResourceError::new(
            "route.key 必须与 menu.key 一致",
            "路由与菜单使用同一个稳定资源键，例如 `order.order`",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    if let Some(icon) = &spec.menu.icon
        && !is_safe_symbol(icon)
    {
        return Err(ResourceError::new(
            format!("menu.icon `{icon}` 格式无效"),
            "只使用字母、数字、下划线、短横线或点",
        )
        .with_resource(resource)
        .with_file(source_path));
    }
    validate_labels(&spec.menu.labels, resource, None, source_path)
}

pub(super) fn validate_extensions(
    resource: &str,
    source_path: &str,
    side: &str,
    extensions: &BTreeMap<String, toml::Value>,
    collect_permissions: bool,
) -> Result<BTreeMap<String, String>, ResourceError> {
    let mut permissions = BTreeMap::new();
    for (key, value) in extensions {
        if !is_snake_identifier(key) {
            return Err(ResourceError::new(
                format!("{side} 扩展键 `{key}` 格式无效"),
                "扩展键使用小写 snake_case；复杂行为放入同名强类型扩展文件",
            )
            .with_resource(resource)
            .with_file(source_path));
        }
        if side == "frontend" && key == "page" {
            let toml::Value::String(page) = value else {
                return Err(ResourceError::new(
                    "frontend 扩展 `page` 必须是字符串",
                    "填写 `@/views/<domain>/<resource>/index.vue` 形式的页面模块路径",
                )
                .with_resource(resource)
                .with_file(source_path));
            };
            let valid = page.starts_with("@/views/")
                && page.ends_with(".vue")
                && page
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || "@/_-.".contains(value));
            if !valid || page.contains("..") {
                return Err(ResourceError::new(
                    format!("frontend 扩展页面路径 `{page}` 格式无效"),
                    "只使用 `@/views/` 下由字母、数字、斜杠、短横线和下划线组成的 Vue 路径",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
        }
        if collect_permissions && let Some(name) = key.strip_suffix("_permission") {
            if name.is_empty() || !is_snake_identifier(name) {
                return Err(ResourceError::new(
                    format!("扩展权限名 `{key}` 格式无效"),
                    "使用 `<action>_permission`，其中 action 为小写 snake_case",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
            let toml::Value::String(permission) = value else {
                return Err(ResourceError::new(
                    format!("扩展权限 `{key}` 必须是字符串"),
                    "填写 `module:resource:action` 形式的权限字符串",
                )
                .with_resource(resource)
                .with_file(source_path));
            };
            if !is_permission(permission) {
                return Err(ResourceError::new(
                    format!("扩展权限 `{key}` 的值格式无效"),
                    "使用 `module:resource-name:action-name` 形式的三段 kebab-case 权限标识",
                )
                .with_resource(resource)
                .with_file(source_path));
            }
            permissions.insert(name.to_owned(), permission.clone());
        }
    }
    Ok(permissions)
}
