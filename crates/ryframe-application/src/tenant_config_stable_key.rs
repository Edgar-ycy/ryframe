pub fn join_path(path: &[String]) -> String {
    path.iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("/")
}

pub fn normalize_stable_key(value: &str) -> String {
    value.to_ascii_lowercase()
}

pub fn normalize_resource_stable_key(resource_type: &str, value: &str) -> String {
    if resource_type == "department" {
        value.to_owned()
    } else {
        normalize_stable_key(value)
    }
}

pub fn normalize_department_path(path: &[String]) -> Vec<String> {
    // 部门名称允许 Unicode；为避免用 Rust 近似 MySQL 排序规则，路径匹配采用明确的二进制语义。
    path.to_vec()
}

pub fn route_menu_key(route_key: &str) -> String {
    format!("route:{}:{route_key}", route_key.len())
}

pub fn action_menu_key(parent_key: &str, permission_code: &str) -> String {
    format!(
        "action:{}:{parent_key}:{}:{permission_code}",
        parent_key.len(),
        permission_code.len()
    )
}

pub fn is_platform_only_permission(code: &str) -> bool {
    let normalized = code.to_ascii_lowercase();
    normalized.starts_with("platform:")
        || normalized == "tenant:*"
        || normalized.starts_with("tenant:")
        || normalized == "monitor:retention:*"
        || normalized.starts_with("monitor:retention:")
}

pub fn permission_contains_wildcard(code: &str) -> bool {
    code.split(':').any(|segment| segment == "*")
}
