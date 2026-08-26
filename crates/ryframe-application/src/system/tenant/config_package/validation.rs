use std::collections::{BTreeMap, BTreeSet};

use ryframe_kernel::{AppError, AppResult};

use super::{
    CONFIG_KEY_MAX_BYTES, CONFIG_NAME_MAX_CHARS, CONFIG_VALUE_MAX_CHARS, ICON_MAX_CHARS,
    MENU_STABLE_KEY_MAX_BYTES, NAME_MAX_CHARS, PERMISSION_CODE_MAX_BYTES, REMARK_MAX_CHARS,
    ROUTE_KEY_MAX_BYTES, STABLE_CODE_MAX_BYTES, TenantConfigPackageLimits,
    TenantConfigPackageResources,
    package::{
        action_menu_stable_key, collation_key, normalized_path_key, route_menu_stable_key,
        unique_by, validate_department_stable_key, validate_optional_text, validate_parent_graph,
        validate_path, validate_stable_code, validate_stable_text, validate_status, validate_text,
    },
};

pub(super) fn validate(
    resources: &TenantConfigPackageResources,
    limits: TenantConfigPackageLimits,
) -> AppResult<()> {
    validate_capacity(resources, limits)?;
    let department_paths = validate_departments(resources)?;
    validate_unique_resource_keys(resources)?;
    validate_dict_data(resources)?;
    let permissions = validate_permissions(resources)?;
    validate_menus(resources, &permissions)?;
    validate_configs(resources)?;
    validate_posts(resources)?;
    validate_dict_types(resources)?;
    validate_dict_data_statuses(resources)?;
    validate_roles(resources, &permissions, &department_paths)
}

fn validate_capacity(
    resources: &TenantConfigPackageResources,
    limits: TenantConfigPackageLimits,
) -> AppResult<()> {
    if resources.counts().total()? > limits.max_items {
        return Err(AppError::Validation(format!(
            "配置包项目数量超过限制（最大 {} 项）",
            limits.max_items
        )));
    }
    Ok(())
}

fn validate_departments(
    resources: &TenantConfigPackageResources,
) -> AppResult<BTreeSet<Vec<String>>> {
    unique_by(
        resources
            .departments
            .iter()
            .map(|item| normalized_path_key(&item.path)),
        "部门完整路径重复",
    )?;
    let paths = resources
        .departments
        .iter()
        .map(|item| normalized_path_key(&item.path))
        .collect::<BTreeSet<_>>();
    for department in &resources.departments {
        validate_path(&department.path, "部门路径")?;
        validate_department_stable_key(&department.path)?;
        validate_status(&department.status, "部门状态")?;
        validate_optional_text(&department.remark, REMARK_MAX_CHARS, "部门备注")?;
        let path = normalized_path_key(&department.path);
        if path.len() > 1 && !paths.contains(&path[..path.len() - 1]) {
            return Err(AppError::Validation(format!(
                "部门路径 {} 缺少父部门",
                department.path.join("/")
            )));
        }
    }
    Ok(paths)
}

fn validate_unique_resource_keys(resources: &TenantConfigPackageResources) -> AppResult<()> {
    unique_by(
        resources.posts.iter().map(|item| collation_key(&item.code)),
        "岗位代码重复",
    )?;
    unique_by(
        resources
            .dict_types
            .iter()
            .map(|item| collation_key(&item.code)),
        "字典类型代码重复",
    )?;
    unique_by(
        resources
            .dict_data
            .iter()
            .map(|item| (collation_key(&item.type_code), collation_key(&item.value))),
        "字典数据稳定键重复",
    )?;
    unique_by(
        resources
            .configs
            .iter()
            .map(|item| collation_key(&item.key)),
        "参数键重复",
    )?;
    unique_by(
        resources
            .permissions
            .iter()
            .map(|item| collation_key(&item.code)),
        "权限代码重复",
    )?;
    unique_by(
        resources
            .menus
            .iter()
            .map(|item| collation_key(&item.stable_key)),
        "菜单稳定键重复",
    )?;
    unique_by(
        resources.roles.iter().map(|item| collation_key(&item.code)),
        "角色代码重复",
    )
}

fn validate_dict_data(resources: &TenantConfigPackageResources) -> AppResult<()> {
    let dict_types = resources
        .dict_types
        .iter()
        .map(|item| collation_key(&item.code))
        .collect::<BTreeSet<_>>();
    for item in &resources.dict_data {
        validate_stable_code(&item.type_code, STABLE_CODE_MAX_BYTES, "字典类型代码")?;
        validate_stable_code(&item.value, STABLE_CODE_MAX_BYTES, "字典数据值")?;
        validate_text(&item.label, NAME_MAX_CHARS, "字典数据标签")?;
        validate_optional_text(&item.css_class, NAME_MAX_CHARS, "字典数据样式")?;
        validate_optional_text(&item.remark, REMARK_MAX_CHARS, "字典数据备注")?;
        if !dict_types.contains(&collation_key(&item.type_code)) {
            return Err(AppError::Validation(format!(
                "字典数据引用了不存在的字典类型：{}",
                item.type_code
            )));
        }
    }
    Ok(())
}

fn validate_permissions(resources: &TenantConfigPackageResources) -> AppResult<BTreeSet<String>> {
    let permissions = resources
        .permissions
        .iter()
        .map(|item| collation_key(&item.code))
        .collect::<BTreeSet<_>>();
    let mut parents = BTreeMap::new();
    for item in &resources.permissions {
        validate_stable_code(&item.code, PERMISSION_CODE_MAX_BYTES, "权限代码")?;
        validate_text(&item.name, NAME_MAX_CHARS, "权限名称")?;
        validate_optional_text(&item.icon, NAME_MAX_CHARS, "权限图标")?;
        validate_status(&item.status, "权限状态")?;
        if !matches!(item.permission_type.as_str(), "api" | "menu") {
            return Err(AppError::Validation(format!(
                "权限 {} 的类型不受支持",
                item.code
            )));
        }
        if permission_contains_wildcard(&item.code) {
            return Err(AppError::Validation("配置包不能包含超级通配权限".into()));
        }
        if let Some(parent) = item.parent_code.as_deref()
            && !permissions.contains(&collation_key(parent))
        {
            return Err(AppError::Validation(format!(
                "权限 {} 引用了不存在的父权限 {}",
                item.code, parent
            )));
        }
        parents.insert(
            collation_key(&item.code),
            item.parent_code.as_deref().map(collation_key),
        );
    }
    validate_parent_graph(&parents, "权限目录")?;
    Ok(permissions)
}

fn validate_menus(
    resources: &TenantConfigPackageResources,
    permissions: &BTreeSet<String>,
) -> AppResult<()> {
    let menus = resources
        .menus
        .iter()
        .map(|item| collation_key(&item.stable_key))
        .collect::<BTreeSet<_>>();
    let menu_types = resources
        .menus
        .iter()
        .map(|item| (collation_key(&item.stable_key), item.menu_type.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut parents = BTreeMap::new();
    for item in &resources.menus {
        validate_menu(item, &menus, &menu_types, permissions)?;
        parents.insert(
            collation_key(&item.stable_key),
            item.parent_stable_key.as_deref().map(collation_key),
        );
    }
    validate_parent_graph(&parents, "菜单目录")
}

fn validate_menu(
    item: &super::PortableMenu,
    menus: &BTreeSet<String>,
    menu_types: &BTreeMap<String, &str>,
    permissions: &BTreeSet<String>,
) -> AppResult<()> {
    validate_stable_text(&item.stable_key, MENU_STABLE_KEY_MAX_BYTES, "菜单稳定键")?;
    validate_text(&item.name, NAME_MAX_CHARS, "菜单名称")?;
    validate_optional_text(&item.icon, ICON_MAX_CHARS, "菜单图标")?;
    validate_optional_text(&item.remark, REMARK_MAX_CHARS, "菜单备注")?;
    validate_status(&item.status, "菜单状态")?;
    if let Some(parent) = item.parent_stable_key.as_deref()
        && !menus.contains(&collation_key(parent))
    {
        return Err(AppError::Validation(format!(
            "菜单 {} 引用了不存在的父菜单 {}",
            item.stable_key, parent
        )));
    }
    if let Some(parent) = item.parent_stable_key.as_deref()
        && menu_types.get(&collation_key(parent)).copied() == Some("F")
    {
        return Err(AppError::Validation(format!(
            "菜单 {} 不能将操作菜单作为父菜单",
            item.stable_key
        )));
    }
    match item.menu_type.as_str() {
        "M" | "C" => validate_route_menu(item, permissions),
        "F" => validate_action_menu(item, permissions),
        _ => Err(AppError::Validation(format!(
            "菜单 {} 的类型不受支持",
            item.stable_key
        ))),
    }
}

fn validate_route_menu(
    item: &super::PortableMenu,
    permissions: &BTreeSet<String>,
) -> AppResult<()> {
    let route_key = item.route_key.as_deref().ok_or_else(|| {
        AppError::Validation(format!("目录或页面菜单 {} 缺少 route_key", item.stable_key))
    })?;
    validate_stable_code(route_key, ROUTE_KEY_MAX_BYTES, "页面 route_key")?;
    if item.stable_key != route_menu_stable_key(route_key) {
        return Err(AppError::Validation(format!(
            "菜单 {} 的稳定键与 route_key 不匹配",
            item.stable_key
        )));
    }
    if item.menu_type == "C" && item.permission_code.is_none() {
        return Err(AppError::Validation(format!(
            "页面菜单 {} 必须绑定权限代码",
            item.stable_key
        )));
    }
    if let Some(permission) = item.permission_code.as_deref()
        && !permissions.contains(&collation_key(permission))
    {
        return Err(AppError::Validation(format!(
            "目录或页面菜单引用了不存在的权限：{}",
            permission
        )));
    }
    Ok(())
}

fn validate_action_menu(
    item: &super::PortableMenu,
    permissions: &BTreeSet<String>,
) -> AppResult<()> {
    if item.route_key.is_some() {
        return Err(AppError::Validation(format!(
            "操作菜单 {} 不能声明 route_key",
            item.stable_key
        )));
    }
    let parent = item
        .parent_stable_key
        .as_deref()
        .ok_or_else(|| AppError::Validation("操作菜单必须声明父菜单稳定键".into()))?;
    let permission = item
        .permission_code
        .as_deref()
        .ok_or_else(|| AppError::Validation("操作菜单必须绑定权限代码".into()))?;
    if !permissions.contains(&collation_key(permission)) {
        return Err(AppError::Validation(format!(
            "操作菜单引用了不存在的权限：{}",
            permission
        )));
    }
    if item.stable_key != action_menu_stable_key(parent, permission) {
        return Err(AppError::Validation(format!(
            "操作菜单 {} 的稳定键不合法",
            item.stable_key
        )));
    }
    Ok(())
}

fn validate_configs(resources: &TenantConfigPackageResources) -> AppResult<()> {
    for item in &resources.configs {
        validate_stable_code(&item.key, CONFIG_KEY_MAX_BYTES, "参数键")?;
        validate_text(&item.name, CONFIG_NAME_MAX_CHARS, "参数名称")?;
        validate_text(&item.value, CONFIG_VALUE_MAX_CHARS, "参数值")?;
        validate_optional_text(&item.remark, REMARK_MAX_CHARS, "参数备注")?;
        if super::is_sensitive_config_key(&item.key) {
            return Err(AppError::Validation(format!(
                "敏感参数不能进入配置包：{}",
                item.key
            )));
        }
    }
    Ok(())
}

fn validate_posts(resources: &TenantConfigPackageResources) -> AppResult<()> {
    for item in &resources.posts {
        validate_stable_code(&item.code, STABLE_CODE_MAX_BYTES, "岗位代码")?;
        validate_text(&item.name, NAME_MAX_CHARS, "岗位名称")?;
        validate_optional_text(&item.remark, REMARK_MAX_CHARS, "岗位备注")?;
        validate_status(&item.status, "岗位状态")?;
    }
    Ok(())
}

fn validate_dict_types(resources: &TenantConfigPackageResources) -> AppResult<()> {
    for item in &resources.dict_types {
        validate_stable_code(&item.code, STABLE_CODE_MAX_BYTES, "字典类型代码")?;
        validate_text(&item.name, NAME_MAX_CHARS, "字典类型名称")?;
        validate_optional_text(&item.remark, REMARK_MAX_CHARS, "字典类型备注")?;
        validate_status(&item.status, "字典类型状态")?;
    }
    Ok(())
}

fn validate_dict_data_statuses(resources: &TenantConfigPackageResources) -> AppResult<()> {
    for item in &resources.dict_data {
        validate_status(&item.status, "字典数据状态")?;
    }
    Ok(())
}

fn validate_roles(
    resources: &TenantConfigPackageResources,
    permissions: &BTreeSet<String>,
    department_paths: &BTreeSet<Vec<String>>,
) -> AppResult<()> {
    for item in &resources.roles {
        validate_role(item, permissions, department_paths)?;
    }
    Ok(())
}

fn validate_role(
    role: &super::PortableRole,
    permissions: &BTreeSet<String>,
    department_paths: &BTreeSet<Vec<String>>,
) -> AppResult<()> {
    validate_stable_code(&role.code, STABLE_CODE_MAX_BYTES, "角色代码")?;
    validate_text(&role.name, NAME_MAX_CHARS, "角色名称")?;
    validate_optional_text(&role.remark, REMARK_MAX_CHARS, "角色备注")?;
    validate_status(&role.status, "角色状态")?;
    if !matches!(role.data_scope.as_str(), "1" | "2" | "3" | "4" | "5") {
        return Err(AppError::Validation(format!(
            "角色 {} 的数据范围不受支持",
            role.code
        )));
    }
    if role.data_scope != "2" && !role.custom_department_paths.is_empty() {
        return Err(AppError::Validation(format!(
            "非自定义数据范围角色 {} 不能包含自定义部门",
            role.code
        )));
    }
    unique_by(
        role.permission_codes.iter().map(|code| collation_key(code)),
        "角色权限代码重复",
    )?;
    unique_by(
        role.custom_department_paths
            .iter()
            .map(|path| normalized_path_key(path)),
        "角色自定义部门路径重复",
    )?;
    validate_role_permissions(role, permissions)?;
    validate_role_departments(role, department_paths)
}

fn validate_role_permissions(
    role: &super::PortableRole,
    permissions: &BTreeSet<String>,
) -> AppResult<()> {
    for code in &role.permission_codes {
        validate_stable_code(code, PERMISSION_CODE_MAX_BYTES, "角色权限代码")?;
        if !permissions.contains(&collation_key(code)) {
            return Err(AppError::Validation(format!(
                "角色 {} 引用了不存在的权限 {}",
                role.code, code
            )));
        }
    }
    Ok(())
}

fn validate_role_departments(
    role: &super::PortableRole,
    department_paths: &BTreeSet<Vec<String>>,
) -> AppResult<()> {
    for path in &role.custom_department_paths {
        validate_path(path, "角色自定义部门路径")?;
        validate_department_stable_key(path)?;
        if !department_paths.contains(&normalized_path_key(path)) {
            return Err(AppError::Validation(format!(
                "角色 {} 引用了不存在的部门路径",
                role.code
            )));
        }
    }
    Ok(())
}

fn permission_contains_wildcard(code: &str) -> bool {
    code.split(':').any(|segment| segment == "*")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::tenant::config_package::{
        PortableDepartment, PortableDictData, PortableMenu, PortablePermission, PortableRole,
    };

    fn limits() -> TenantConfigPackageLimits {
        TenantConfigPackageLimits::new(1024, 2048, 100).expect("测试容量限制应当有效")
    }

    fn permission(code: &str, parent_code: Option<&str>) -> PortablePermission {
        PortablePermission {
            code: code.into(),
            name: code.into(),
            parent_code: parent_code.map(str::to_owned),
            permission_type: "api".into(),
            icon: None,
            sort: 0,
            status: "1".into(),
        }
    }

    fn menu(stable_key: &str, menu_type: &str, parent: Option<&str>) -> PortableMenu {
        PortableMenu {
            stable_key: stable_key.into(),
            parent_stable_key: parent.map(str::to_owned),
            name: stable_key.into(),
            menu_type: menu_type.into(),
            permission_code: None,
            route_key: None,
            icon: None,
            sort: 0,
            visible: true,
            status: "1".into(),
            remark: None,
        }
    }

    fn validation_message(resources: &TenantConfigPackageResources) -> String {
        match validate(resources, limits()).expect_err("测试资源应当校验失败") {
            AppError::Validation(message) => message,
            error => panic!("预期参数校验错误，实际为 {error}"),
        }
    }

    #[test]
    fn rejects_missing_dict_type_reference() {
        let resources = TenantConfigPackageResources {
            dict_data: vec![PortableDictData {
                type_code: "unknown".into(),
                value: "value".into(),
                label: "标签".into(),
                sort: 0,
                status: "1".into(),
                css_class: None,
                remark: None,
            }],
            ..Default::default()
        };

        assert_eq!(
            validation_message(&resources),
            "字典数据引用了不存在的字典类型：unknown"
        );
    }

    #[test]
    fn rejects_permission_parent_cycle() {
        let resources = TenantConfigPackageResources {
            permissions: vec![
                permission("system:a", Some("system:b")),
                permission("system:b", Some("system:a")),
            ],
            ..Default::default()
        };

        assert_eq!(validation_message(&resources), "权限目录存在循环引用");
    }

    #[test]
    fn rejects_action_menu_as_parent_before_child_type_validation() {
        let route_key = "parent";
        let route_stable_key = route_menu_stable_key(route_key);
        let permission_code = "system:read";
        let action_stable_key = action_menu_stable_key(&route_stable_key, permission_code);
        let mut route = menu(&route_stable_key, "M", None);
        route.route_key = Some(route_key.into());
        let mut action = menu(&action_stable_key, "F", Some(&route_stable_key));
        action.permission_code = Some(permission_code.into());
        let resources = TenantConfigPackageResources {
            permissions: vec![permission(permission_code, None)],
            menus: vec![
                route,
                action,
                menu("child", "invalid", Some(&action_stable_key)),
            ],
            ..Default::default()
        };

        assert_eq!(
            validation_message(&resources),
            "菜单 child 不能将操作菜单作为父菜单"
        );
    }

    #[test]
    fn rejects_menu_parent_cycle() {
        let mut first = menu("route:1:a", "M", Some("route:1:b"));
        first.route_key = Some("a".into());
        let mut second = menu("route:1:b", "M", Some("route:1:a"));
        second.route_key = Some("b".into());
        let resources = TenantConfigPackageResources {
            menus: vec![first, second],
            ..Default::default()
        };

        assert_eq!(validation_message(&resources), "菜单目录存在循环引用");
    }

    #[test]
    fn rejects_role_permission_and_department_references() {
        let department = PortableDepartment {
            path: vec!["总部".into()],
            sort: 0,
            status: "1".into(),
            remark: None,
        };
        let role = PortableRole {
            code: "operator".into(),
            name: "操作员".into(),
            data_scope: "2".into(),
            status: "1".into(),
            sort: 0,
            remark: None,
            permission_codes: vec!["system:missing".into()],
            custom_department_paths: vec![vec!["总部".into(), "缺失部门".into()]],
        };
        let resources = TenantConfigPackageResources {
            departments: vec![department],
            roles: vec![role],
            ..Default::default()
        };
        assert_eq!(
            validation_message(&resources),
            "角色 operator 引用了不存在的权限 system:missing"
        );

        let mut department_resources = resources;
        department_resources.roles[0].permission_codes.clear();
        assert_eq!(
            validation_message(&department_resources),
            "角色 operator 引用了不存在的部门路径"
        );
    }
}
