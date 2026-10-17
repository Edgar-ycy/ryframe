use ryframe_db::migration::compile_business_access_catalog;

const ORDER: &str = include_str!("../../../ryframe-sdk/tests/fixtures/order_access.toml");

fn global() -> String {
    ORDER.replace(
        "module = \"business\"",
        "module = \"business\"\nsystem_tenant_only = true",
    )
}

fn item() -> String {
    ORDER
        .replace("\"order\"", "\"order_item\"")
        .replace("order.order", "order.order-item")
        .replace("order:order:", "order:order-item:")
        .replace("/order/order", "/order/order_item")
        .replace("parent = \"order_item\"", "parent = \"order\"")
}

#[test]
fn business_access_compiles_one_directory_and_unique_crud_codes() {
    let catalog = compile_business_access_catalog([("order", ORDER)]).unwrap();
    assert_eq!(catalog.resources().count(), 1);
    assert_eq!(catalog.permission_names().len(), 4);
    assert_eq!(catalog.permission_names()["order:order:list"], "订单：查询");
    assert!(catalog.system_permissions().is_empty());
    assert_eq!(catalog.menus().len(), 2);
    assert_eq!(catalog.menus()[0].menu_type, "M");
    assert_eq!(catalog.menus()[1].parent_route_key(), Some("order"));
    assert_eq!(catalog.menus()[0].icon(), Some("FolderOpened"));
}

#[test]
fn mixed_scope_directory_is_visible_but_global_page_and_codes_remain_system_only() {
    let global = global();
    let alone = compile_business_access_catalog([("order", global.as_str())]).unwrap();
    assert!(alone.system_routes().contains("order"));
    assert!(alone.system_routes().contains("order.order"));
    assert_eq!(alone.system_permissions().len(), 4);
    let batch = format!("{global}\n{}", item().strip_prefix("version = 1").unwrap());
    let mixed = compile_business_access_catalog([("order", batch.as_str())]).unwrap();
    assert!(!mixed.system_routes().contains("order"));
    assert!(mixed.system_routes().contains("order.order"));
    assert!(!mixed.system_routes().contains("order.order-item"));
    assert_eq!(mixed.menus().len(), 3);
    assert_eq!(mixed.permission_names().len(), 8);
    assert_eq!(mixed.system_permissions().len(), 4);
}

#[test]
fn existing_core_parent_is_reused_without_duplicate_directory() {
    let source = ORDER.replace("parent = \"order\"", "parent = \"system\"");
    let catalog = compile_business_access_catalog([("order", source.as_str())]).unwrap();
    assert_eq!(catalog.menus().len(), 1);
    assert_eq!(catalog.menus()[0].parent_route_key(), Some("system"));
}

#[test]
fn business_catalog_rejects_all_conflicts_before_installing_anything() {
    let variants = [
        ORDER.replace("version = 1", "version = 2"),
        ORDER.replace("module = \"business\"", "module = \"system\""),
        ORDER.replace("name = \"order\"", "name = \"post\""),
        ORDER.replace("order.order", "system.post"),
        ORDER.replace("order:order:", "system:post:"),
        ORDER.replace("parent = \"order\"", "parent = \"absent\""),
        ORDER.replace("parent = \"order\"", "parent = \"system.post\""),
        ORDER.replace(
            "module = \"business\"",
            "module = \"business\"\ncapability = \"demo.optional\"",
        ),
        ORDER.replace(
            "module = \"business\"",
            "module = \"business\"\nunknown_field = true",
        ),
    ];
    for (index, source) in variants.iter().enumerate() {
        assert!(
            compile_business_access_catalog([("order", source.as_str())]).is_err(),
            "冲突场景 {index}"
        );
    }
    assert!(compile_business_access_catalog([("order", ORDER), ("order", ORDER)]).is_err());
    assert!(compile_business_access_catalog([("other", ORDER)]).is_err());
    assert!(compile_business_access_catalog([("Order", ORDER)]).is_err());
    let duplicated = format!("{ORDER}\n{}", ORDER.strip_prefix("version = 1").unwrap());
    assert!(compile_business_access_catalog([("order", duplicated.as_str())]).is_err());
    assert!(compile_business_access_catalog([("order", ORDER)]).is_ok());
}

#[test]
fn display_labels_remain_lossless_and_extension_permissions_are_catalogued() {
    let source = ORDER.replace("订单", "O'Reilly\\\\路径");
    let source = format!("{source}\nexport = \"order:order:export\"\n");
    let catalog = compile_business_access_catalog([("order", source.as_str())]).unwrap();
    assert_eq!(catalog.menus()[1].name, "O'Reilly\\路径");
    assert_eq!(
        catalog.permission_names()["order:order:export"],
        "O'Reilly\\路径：扩展"
    );
    assert_eq!(
        catalog
            .resource("order", "order")
            .unwrap()
            .extension_permissions["export"],
        "order:order:export"
    );
}
