use ryframe_sdk::{
    BusinessModuleBuilder, ResourceDatabase, ResourceModel, RyFrameBusinessModule, validate_modules,
};

#[derive(ResourceModel)]
#[resource(name = "order", table = "biz_order", database = "tenant")]
pub struct Order {
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(primary_key)]
    pub id: i64,
    #[resource(rename_from = "title")]
    pub name: String,
}

#[test]
fn derive_exports_rust_resource_descriptor() {
    let descriptor = Order::descriptor();
    assert_eq!(descriptor.name, "order");
    assert_eq!(descriptor.table, "biz_order");
    assert_eq!(descriptor.database, ResourceDatabase::Tenant);
    assert_eq!(descriptor.fields.len(), 3);
    assert_eq!(descriptor.fields[2].rename_from, Some("title"));
}

#[test]
fn explicit_registry_rejects_missing_dependency_and_cycle() {
    let missing = module("sales", &["inventory"]);
    let error = validate_modules(&[missing]).expect_err("未注册依赖必须失败");
    assert!(error.to_string().contains("未注册模块 inventory"));

    let sales = module("sales", &["inventory"]);
    let inventory = module("inventory", &["sales"]);
    let error = validate_modules(&[sales, inventory]).expect_err("循环依赖必须失败");
    assert!(error.to_string().contains("依赖存在循环"));
}

fn module(name: &'static str, dependencies: &'static [&'static str]) -> RyFrameBusinessModule {
    BusinessModuleBuilder::new(name)
        .dependencies(dependencies)
        .build()
}
