#![cfg(feature = "api")]

use ryframe_sdk::{BusinessModuleBuilder, OpenApiDocument, compose_openapi, serde_json::json};

fn business_document() -> OpenApiDocument {
    let mut document = OpenApiDocument::default();
    document.extensions.get_or_insert_default().insert(
        "x-ryframe-crud-resources".into(),
        json!({"version": 1, "resources": [{
            "name": "order", "route": {"key": "order.order"},
            "menu": {"labels": {"zh_cn": "订单"}},
            "access": {"permissions": {
                "list": "order:order:list", "read": "order:order:list",
                "create": "order:order:add"
            }}
        }]}),
    );
    document
}

#[test]
fn composed_business_resources_keep_core_catalog_and_add_authoritative_access() {
    let module = BusinessModuleBuilder::new("order")
        .openapi(business_document)
        .build();
    let document = compose_openapi(ryframe_api::openapi::document(), &[module])
        .expect("业务资源应补齐权限与菜单目录");
    let extensions = document.extensions.expect("应保留契约扩展");
    let codes = extensions["x-ryframe-permission-catalog"]["codes"]
        .as_array()
        .expect("权限目录应为数组");
    assert!(codes.contains(&json!("system:post:list")));
    assert!(codes.contains(&json!("order:order:add")));
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == json!("order:order:list"))
            .count(),
        1
    );
    let routes = extensions["x-ryframe-menu-routes"]["routes"]
        .as_array()
        .expect("菜单目录应为数组");
    let route = routes
        .iter()
        .find(|route| route["route_key"] == "order.order")
        .expect("业务菜单必须存在");
    assert_eq!(route["title_key"], "order");
    assert_eq!(route["permission_code"], "order:order:list");
    assert!(route["capability_code"].is_null());
}

#[test]
fn duplicate_business_routes_fail_closed() {
    let modules = [
        BusinessModuleBuilder::new("first")
            .openapi(business_document)
            .build(),
        BusinessModuleBuilder::new("second")
            .openapi(business_document)
            .build(),
    ];
    let error = compose_openapi(ryframe_api::openapi::document(), &modules)
        .err()
        .expect("重复业务菜单不能覆盖既有菜单");
    assert!(error.to_string().contains("重复菜单路由"));
}

fn invalid_business_document() -> OpenApiDocument {
    let mut document = business_document();
    document
        .extensions
        .as_mut()
        .expect("应存在资源目录")
        .get_mut("x-ryframe-crud-resources")
        .expect("应存在资源元数据")["resources"][0]["access"]["permissions"]["create"] =
        json!(null);
    document
}

#[test]
fn malformed_business_permission_fails_closed() {
    let module = BusinessModuleBuilder::new("order")
        .openapi(invalid_business_document)
        .build();
    let error = compose_openapi(ryframe_api::openapi::document(), &[module])
        .err()
        .expect("非法业务权限不能进入契约");
    assert!(error.to_string().contains("业务资源契约无效：权限"));
}
