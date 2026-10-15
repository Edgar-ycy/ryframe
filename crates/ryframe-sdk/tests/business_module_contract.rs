use ryframe_sdk::{
    BusinessModuleBuilder, ResourceDatabase, ResourceModel, RyFrameBusinessModule,
    TenantDataTableDescriptor, validate_modules,
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

const TABLE: TenantDataTableDescriptor = TenantDataTableDescriptor {
    table: "biz_order",
    copy_order: 1,
    tenant_column: "tenant_id",
    primary_key_cursor_columns: &["tenant_id", "id"],
    checksum_columns: &["tenant_id", "id", "name"],
    column_types: &["varchar", "bigint", "varchar"],
    has_generated_columns: false,
    foreign_key_dependencies: &[],
    foreign_keys: &[],
    schema_canonical: "order-schema",
};

#[test]
fn tenant_resources_require_complete_matching_catalog() {
    let descriptor = Order::descriptor();
    let missing = BusinessModuleBuilder::new("order")
        .resources([descriptor])
        .build();
    assert!(validate_modules(&[missing]).is_err());
    let complete = BusinessModuleBuilder::new("order")
        .resources([descriptor])
        .tenant_data_tables(&[TABLE])
        .build();
    assert!(validate_modules(&[complete]).is_ok());
    let incomplete = BusinessModuleBuilder::new("order")
        .resources([descriptor])
        .tenant_data_tables(&[TenantDataTableDescriptor {
            checksum_columns: &["tenant_id", "id"],
            ..TABLE
        }])
        .build();
    assert!(validate_modules(&[incomplete]).is_err());
    let duplicate = BusinessModuleBuilder::new("order")
        .resources([descriptor])
        .tenant_data_tables(&[TABLE, TABLE])
        .build();
    assert!(validate_modules(&[duplicate]).is_err());
}

#[derive(ResourceModel)]
#[resource(name = "global_item", table = "global_item", database = "control")]
pub struct GlobalItem {
    #[resource(primary_key)]
    pub id: i64,
    pub name: String,
}

#[test]
fn control_resources_require_complete_current_schema_catalog_without_migration_features() {
    use ryframe_sdk::BusinessControlTableDescriptor;
    let descriptor = GlobalItem::descriptor();
    let table = BusinessControlTableDescriptor {
        table: "global_item",
        schema_canonical: "current-schema",
    };
    let missing = BusinessModuleBuilder::new("global")
        .resources([descriptor])
        .build();
    assert!(
        validate_modules(&[missing])
            .unwrap_err()
            .to_string()
            .contains("控制库资源")
    );
    let complete = BusinessModuleBuilder::new("global")
        .resources([descriptor])
        .control_tables(&[BusinessControlTableDescriptor {
            table: "global_item",
            schema_canonical: "current-schema",
        }])
        .build();
    assert!(validate_modules(&[complete]).is_ok());
    for tables in [
        vec![table, table],
        vec![BusinessControlTableDescriptor {
            schema_canonical: "",
            ..table
        }],
        vec![BusinessControlTableDescriptor {
            table: "other",
            ..table
        }],
    ] {
        let tables = Box::leak(tables.into_boxed_slice());
        let invalid = BusinessModuleBuilder::new("global")
            .resources([descriptor])
            .control_tables(tables)
            .build();
        assert!(validate_modules(&[invalid]).is_err());
    }
}
