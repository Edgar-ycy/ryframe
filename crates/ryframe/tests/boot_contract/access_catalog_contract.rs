use ryframe_api::permission_catalog::{menu_routes, permission_codes};
use ryframe_db::migration::{access_menus, access_permission_codes};

#[test]
fn api_and_database_seed_use_the_same_merged_access_catalog() {
    let seeded_permissions = access_permission_codes().expect("数据库种子权限目录应可解析");
    assert_eq!(
        seeded_permissions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        permission_codes()
    );

    let seeded_menus = access_menus().expect("数据库种子菜单目录应可解析");
    let api_menus = menu_routes();
    assert_eq!(seeded_menus.len(), api_menus.len());
    for (seeded, api) in seeded_menus.iter().zip(api_menus) {
        assert_eq!(seeded.route_key, api.route_key);
        assert_eq!(seeded.name, api.name);
        assert_eq!(seeded.menu_type, api.menu_type);
        assert_eq!(seeded.permission.as_deref(), api.permission_code);
    }
}

#[test]
fn api_and_worker_build_the_same_tenant_config_target_catalog() {
    let api = ryframe_api::tenant_config_target_catalog().expect("API 目标目录应有效");
    let worker = ryframe::boot::access_catalog::tenant_config_target_catalog()
        .expect("Worker 目标目录应有效");
    assert_eq!(worker.page_routes(), api.page_routes());
    assert_eq!(worker.api_permission_codes(), api.api_permission_codes());
}

#[test]
fn post_access_appears_exactly_once_in_both_consumers() {
    for permission in [
        "system:post:add",
        "system:post:edit",
        "system:post:export",
        "system:post:list",
        "system:post:remove",
    ] {
        assert_eq!(
            permission_codes()
                .iter()
                .filter(|candidate| **candidate == permission)
                .count(),
            1
        );
        assert_eq!(
            access_permission_codes()
                .expect("数据库种子权限目录应可解析")
                .iter()
                .filter(|candidate| **candidate == permission)
                .count(),
            1
        );
    }

    assert_eq!(
        menu_routes()
            .iter()
            .filter(|menu| menu.route_key == "system.post")
            .count(),
        1
    );
    assert_eq!(
        access_menus()
            .expect("数据库种子菜单目录应可解析")
            .iter()
            .filter(|menu| menu.route_key == "system.post")
            .count(),
        1
    );
}
