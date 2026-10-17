#![cfg(feature = "api")]

use ryframe_sdk::{
    BusinessModuleBuilder, OpenApiDocument, ResourceDatabase, ResourceDescriptor, ResourceModel,
    RyFrameBusinessModule, compose_openapi,
    serde_json::{Value, json},
};

const ORDER: &str = include_str!("fixtures/order_access.toml");
const ITEM: &str = include_str!("fixtures/item_access.toml");
const COLLISION: &str = include_str!("fixtures/colliding_access.toml");

#[derive(ResourceModel)]
#[resource(
    name = "order",
    title = "订单",
    database = "tenant",
    table = "biz_order"
)]
pub struct OrderModel {
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(primary_key)]
    pub id: i64,
    pub name: String,
}

fn resource(name: &'static str, table: &'static str) -> ResourceDescriptor {
    ResourceDescriptor {
        name,
        table,
        ..OrderModel::descriptor()
    }
}

fn module(
    name: &'static str,
    access: &'static str,
    resources: Vec<ResourceDescriptor>,
    document: fn() -> OpenApiDocument,
) -> RyFrameBusinessModule {
    BusinessModuleBuilder::new(name)
        .resources(resources)
        .access_catalog(access)
        .openapi(document)
        .build()
}

fn document(module: &str, source: &str) -> OpenApiDocument {
    let access =
        ryframe_db::migration::compile_business_access_catalog([(module, source)]).unwrap();
    let resources = access.resources().map(|binding| {
        let menu = access.menus().iter().find(|menu| menu.route_key == binding.route_key).unwrap();
        let mut authorization = json!({"permissions": {
            "create": binding.permissions[0], "read": binding.permissions[1], "list": binding.permissions[2],
            "update": binding.permissions[3], "delete": binding.permissions[4],
        }});
        if binding.system_tenant_only { authorization["system_tenant_only"] = json!(true); }
        json!({"name": binding.resource, "route": {"key": binding.route_key, "path": binding.route_path},
            "menu": {"key": binding.route_key, "parent": binding.parent, "labels": {"zh_cn": menu.name}},
            "access": authorization, "extension_permissions": binding.extension_permissions})
    }).collect::<Vec<_>>();
    let mut document = OpenApiDocument::default();
    document.extensions.get_or_insert_default().insert(
        "x-ryframe-crud-resources".into(),
        json!({"version":1,"resources":resources}),
    );
    document
}

fn business_document() -> OpenApiDocument {
    document("order", ORDER)
}
fn item_document() -> OpenApiDocument {
    document("sales-orders", ITEM)
}
fn colliding_title_document() -> OpenApiDocument {
    document("order", COLLISION)
}

fn first_resource(document: &mut OpenApiDocument) -> &mut Value {
    &mut document
        .extensions
        .as_mut()
        .unwrap()
        .get_mut("x-ryframe-crud-resources")
        .unwrap()["resources"][0]
}

#[test]
fn composed_business_resources_keep_core_catalog_and_add_authoritative_access() {
    let module = module(
        "order",
        ORDER,
        vec![resource("order", "biz_order")],
        business_document,
    );
    let document = compose_openapi(ryframe_api::openapi::document(), &[module]).unwrap();
    let extensions = document.extensions.unwrap();
    let codes = extensions["x-ryframe-permission-catalog"]["codes"]
        .as_array()
        .unwrap();
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
        .unwrap();
    let page = routes
        .iter()
        .find(|route| route["route_key"] == "order.order")
        .unwrap();
    assert_eq!(page["title_key"], "order");
    assert_eq!(page["permission_code"], "order:order:list");
    assert!(page["capability_code"].is_null());
    let directory = routes
        .iter()
        .find(|route| route["route_key"] == "order")
        .unwrap();
    assert_eq!(directory["menu_type"], "M");
    assert_eq!(directory["title_key"], "orderBusinessDirectory");
    assert_eq!(directory["icon"], "FolderOpened");
    assert!(directory["page_key"].is_null());
    assert!(directory["permission_code"].is_null());
}

#[test]
fn duplicate_business_routes_fail_closed() {
    let modules = [
        module(
            "first",
            ORDER,
            vec![resource("order", "biz_first")],
            business_document,
        ),
        module(
            "second",
            ORDER,
            vec![resource("order", "biz_second")],
            business_document,
        ),
    ];
    let error = compose_openapi(ryframe_api::openapi::document(), &modules)
        .err()
        .expect("重复业务路由不能覆盖目录");
    assert!(error.to_string().contains("路由"));
}

fn invalid_business_document() -> OpenApiDocument {
    let mut document = business_document();
    first_resource(&mut document)["access"]["permissions"]["create"] = Value::Null;
    document
}

#[test]
fn malformed_business_permission_fails_closed() {
    let error = compose_openapi(
        ryframe_api::openapi::document(),
        &[module(
            "order",
            ORDER,
            vec![resource("order", "biz_order")],
            invalid_business_document,
        )],
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("权限"));
}

#[test]
fn snake_case_business_names_generate_valid_frontend_title_keys() {
    let document = compose_openapi(
        ryframe_api::openapi::document(),
        &[module(
            "sales-orders",
            ITEM,
            vec![resource("order_item", "biz_order_item")],
            item_document,
        )],
    )
    .unwrap();
    let extensions = document.extensions.unwrap();
    let routes = extensions["x-ryframe-menu-routes"]["routes"]
        .as_array()
        .unwrap();
    let route = routes
        .iter()
        .find(|route| route["route_key"] == "sales-orders.order-item")
        .unwrap();
    assert_eq!(route["title_key"], "orderItem");
}

#[test]
fn normalized_title_key_collisions_fail_before_overwriting_translation() {
    let error = compose_openapi(
        ryframe_api::openapi::document(),
        &[module(
            "order",
            COLLISION,
            vec![
                resource("order_1", "biz_first"),
                resource("order1", "biz_second"),
            ],
            colliding_title_document,
        )],
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("标题键冲突：order1"));
}

fn invalid_route_document() -> OpenApiDocument {
    let mut document = item_document();
    first_resource(&mut document)["route"]["key"] = json!("sales_orders.order_item");
    document
}

#[test]
fn invalid_menu_keys_fail_during_backend_composition() {
    let error = compose_openapi(
        ryframe_api::openapi::document(),
        &[module(
            "sales-orders",
            ITEM,
            vec![resource("order_item", "biz_order_item")],
            invalid_route_document,
        )],
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("路由") && error.contains("不一致"));
}

fn changed_boundary_document() -> OpenApiDocument {
    let mut document = business_document();
    first_resource(&mut document)["access"]["system_tenant_only"] = json!(true);
    document
}

fn invalid_boundary_document() -> OpenApiDocument {
    let mut document = business_document();
    first_resource(&mut document)["access"]["system_tenant_only"] = json!("false");
    document
}

#[test]
fn omitted_extra_and_conflicting_access_definitions_fail_closed() {
    for module in [
        module(
            "order",
            "version=1",
            vec![resource("order", "biz_order")],
            business_document,
        ),
        module("order", ORDER, vec![], business_document),
        module(
            "order",
            ORDER,
            vec![ResourceDescriptor {
                database: ResourceDatabase::Control,
                ..resource("order", "biz_order")
            }],
            business_document,
        ),
        module(
            "order",
            ORDER,
            vec![resource("order", "biz_order")],
            changed_boundary_document,
        ),
        module(
            "order",
            ORDER,
            vec![resource("order", "biz_order")],
            invalid_boundary_document,
        ),
    ] {
        assert!(compose_openapi(ryframe_api::openapi::document(), &[module]).is_err());
    }
}

#[test]
fn empty_modules_keep_the_official_document_unchanged() {
    let original = ryframe_api::openapi::document();
    let after = compose_openapi(
        original.clone(),
        &[BusinessModuleBuilder::new("empty").build()],
    )
    .unwrap();
    assert_eq!(
        original.to_pretty_json().unwrap(),
        after.to_pretty_json().unwrap()
    );
}
