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
fn rejects_platform_permissions_and_routes() {
    for code in [
        "tenant:read",
        "platform:product:list",
        "platform:config-transfer:list",
        "monitor:server:list",
    ] {
        let resources = TenantConfigPackageResources {
            permissions: vec![permission(code, None)],
            ..Default::default()
        };
        assert_eq!(validation_message(&resources), "配置包不能包含平台专属权限");
    }
    for route_key in ["platform", "platform.tenants", "platform.config-transfer"] {
        let mut route = menu(&route_menu_stable_key(route_key), "M", None);
        route.route_key = Some(route_key.into());
        let resources = TenantConfigPackageResources {
            menus: vec![route],
            ..Default::default()
        };
        assert_eq!(validation_message(&resources), "配置包不能包含平台专属菜单");
    }
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
