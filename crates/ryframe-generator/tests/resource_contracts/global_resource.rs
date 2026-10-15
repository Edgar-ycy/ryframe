use ryframe_generator::{
    IdGeneration, ResourceSpec, StorageKind, normalize_resource, render_resources,
};

fn global_spec() -> ResourceSpec {
    let mut spec =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    spec.storage.kind = StorageKind::ControlRow;
    spec.storage.tenant_field = None;
    spec.storage.configuration_versioned = false;
    spec.database.primary_key = vec!["id".into()];
    spec.database.id_generation = IdGeneration::AutoIncrement;
    spec.fields.retain(|field| field.name != "tenant_id");
    for index in &mut spec.database.indexes {
        index.fields.retain(|field| field != "tenant_id");
    }
    spec.database
        .indexes
        .retain(|index| !index.fields.is_empty());
    spec.access.system_tenant_only = true;
    spec
}

#[test]
fn global_business_keeps_original_columns_and_guards_all_entry_points() {
    let ir = normalize_resource(global_spec(), "order.toml", "global").unwrap();
    let generated = render_resources(&[ir]).unwrap();
    let asset = |suffix: &str| {
        generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with(suffix))
            .unwrap()
            .content
            .as_str()
    };
    let migration = asset("order/migration.rs");
    assert!(!migration.contains("tenant_id"));
    assert!(!migration.contains("fk_order_tenant"));
    let service = asset("order/service.rs");
    assert_eq!(
        service
            .matches("ensure_system_tenant_id(tenant_id)?")
            .count(),
        5
    );
    let repository = asset("order/repository.rs");
    assert_eq!(
        repository
            .matches("ensure_system_tenant_id(tenant_id)?")
            .count(),
        3
    );
    assert!(!repository.contains("Column::TenantId"));
    assert!(!repository.contains("record.tenant_id"));
    assert!(repository.contains("LAST_INSERT_ID()"));
    assert!(asset("order/registration.ts").contains("requiresSystemTenant: true"));
    assert_eq!(
        asset("order/fake.rs")
            .matches("ensure_system_tenant_id(tenant_id)?")
            .count(),
        3
    );
    for asset in generated
        .assets
        .iter()
        .filter(|asset| asset.path.ends_with(".rs"))
    {
        syn::parse_file(&asset.content).unwrap_or_else(|error| panic!("{}: {error}", asset.path));
    }
}

#[test]
fn global_access_must_be_explicit_and_cannot_relabel_tenant_business() {
    let mut spec = global_spec();
    spec.resource.module = "business".into();
    spec.api.path = "/api/v1/business/orders".into();
    spec.access.system_tenant_only = false;
    assert!(
        normalize_resource(spec, "order.toml", "unscoped")
            .unwrap_err()
            .to_string()
            .contains("显式限制")
    );
    let mut spec =
        ResourceSpec::parse(include_str!("../fixtures/order.toml"), "order.toml").unwrap();
    spec.access.system_tenant_only = true;
    assert!(
        normalize_resource(spec, "order.toml", "tenant-global")
            .unwrap_err()
            .to_string()
            .contains("全局资源")
    );
}

#[test]
fn missing_audit_columns_keep_crud_and_request_audit_without_inventing_timestamps() {
    for soft_delete in [false, true] {
        let mut spec = global_spec();
        spec.database.audit = None;
        spec.fields
            .retain(|field| !matches!(field.name.as_str(), "created_at" | "updated_at"));
        if !soft_delete {
            spec.database.soft_delete = None;
            spec.fields.retain(|field| field.name != "del_flag");
        }
        let generated =
            render_resources(&[normalize_resource(spec, "order.toml", "no-audit").unwrap()])
                .unwrap();
        let service = &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with("order/service.rs"))
            .unwrap()
            .content;
        assert!(!service.contains("Utc") && !service.contains("let now"));
        assert!(service.contains("TransactionAuditMode::CurrentRequest"));
        let repository = &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with("order/repository.rs"))
            .unwrap()
            .content;
        assert!(!repository.contains("Utc::now()"));
        assert_eq!(repository.contains("active.del_flag = Set("), soft_delete);
        let migration = &generated
            .assets
            .iter()
            .find(|asset| asset.path.ends_with("order/migration.rs"))
            .unwrap()
            .content;
        assert!(!migration.contains("created_at") && !migration.contains("updated_at"));
    }
}

#[test]
fn empty_and_numeric_filters_do_not_declare_borrowed_lifetimes() {
    for numeric_filter in [false, true] {
        let mut spec = global_spec();
        spec.database.soft_delete = None;
        spec.fields.retain(|field| field.name != "del_flag");
        spec.database.indexes.clear();
        for field in &mut spec.fields {
            field.usage.filter = numeric_filter && field.name == "status";
        }
        let generated =
            render_resources(&[normalize_resource(spec, "order.toml", "no-borrows").unwrap()])
                .unwrap();
        for path in [
            "order/model.rs",
            "order/port.rs",
            "order/repository.rs",
            "order/fake.rs",
        ] {
            let content = &generated
                .assets
                .iter()
                .find(|asset| asset.path.ends_with(path))
                .unwrap()
                .content;
            assert!(
                !content.contains("OrderFilter<"),
                "{path}: 无借用字段不应声明生命周期"
            );
            if path.ends_with("repository.rs") || path.ends_with("fake.rs") {
                assert_eq!(content.contains("_filter: OrderFilter"), !numeric_filter);
            }
            if path.ends_with("repository.rs") {
                assert_eq!(content.contains("ColumnTrait"), numeric_filter);
            }
        }
    }
}
