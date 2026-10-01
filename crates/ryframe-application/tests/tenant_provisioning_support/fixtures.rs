use ryframe_application::ports::tenants::{
    TenantAuthorizationTemplate, TenantBaseCatalogTemplate, TenantDictionaryDataTemplate,
    TenantDictionaryTypeTemplate, TenantMenuTemplate, TenantPermissionTemplate,
    TenantProvisioningTemplate,
};

pub(super) fn policy_template() -> TenantProvisioningTemplate {
    TenantProvisioningTemplate {
        authorization: TenantAuthorizationTemplate {
            permissions: vec![
                permission(1, "system:root", None),
                // 普通目录保留结构闭包，平台权限始终排除。
                permission(2, "system:identity", Some(1)),
                permission(3, "system:user:list", Some(2)),
                permission(4, "system:user:add", Some(1)),
                permission(5, "monitor:job:list", Some(1)),
                permission(20, "*:*:*", None),
                permission(21, "tenant:read", None),
                permission(22, "platform:read", None),
                permission(23, "monitor:retention:execute", None),
                permission(24, "system:config-transfer:list", Some(1)),
            ],
            menus: vec![
                menu(1, None),
                menu(2, Some("platform")),
                menu(3, Some("platform.users")),
                menu(4, Some("monitor.retention")),
                menu(5, Some("system.config-transfer")),
                menu(6, Some("system.user")),
            ],
        },
        base_catalogs: TenantBaseCatalogTemplate {
            dictionary_types: vec![TenantDictionaryTypeTemplate {
                name: "有效字典".into(),
                code: "active".into(),
                status: "1".into(),
                remark: None,
                delete_flag: "0".into(),
            }],
            dictionary_data: vec![dictionary_data("active"), dictionary_data("orphan")],
            ..TenantBaseCatalogTemplate::default()
        },
    }
}

fn permission(id: i64, code: &str, parent: Option<i64>) -> TenantPermissionTemplate {
    TenantPermissionTemplate {
        source_id: id,
        name: code.into(),
        code: code.into(),
        parent_source_id: parent,
        permission_type: "api".into(),
        icon: None,
        sort: id as i32,
        status: "1".into(),
        assign_admin: false,
        assign_user: false,
    }
}

fn menu(id: i64, route_key: Option<&str>) -> TenantMenuTemplate {
    TenantMenuTemplate {
        source_id: id,
        name: format!("菜单 {id}"),
        parent_source_id: None,
        menu_type: "C".into(),
        permission_source_id: None,
        route_key: route_key.map(str::to_owned),
        icon: None,
        sort: id as i32,
        visible: true,
        status: "1".into(),
        remark: None,
        delete_flag: "0".into(),
    }
}

fn dictionary_data(type_code: &str) -> TenantDictionaryDataTemplate {
    TenantDictionaryDataTemplate {
        type_code: type_code.into(),
        label: type_code.into(),
        value: "1".into(),
        sort: 1,
        status: "1".into(),
        css_class: None,
        remark: None,
        delete_flag: "0".into(),
    }
}
