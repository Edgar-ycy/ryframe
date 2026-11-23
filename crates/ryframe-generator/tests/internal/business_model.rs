use super::*;

fn package() -> BusinessPackage {
    BusinessPackage {
        name: "order-business".into(),
        module: "order".into(),
        root: std::path::PathBuf::from("order-business"),
        workspace_root: std::path::PathBuf::from("."),
    }
}

fn field(name: &str, rust_type: &str, primary_key: bool) -> OwnedFieldDescriptor {
    OwnedFieldDescriptor {
        name: name.into(),
        column: name.into(),
        rust_type: rust_type.into(),
        sql_type: None,
        sql_default: None,
        sql_on_update: None,
        sql_comment: None,
        collation: None,
        rename_from: None,
        default: None,
        primary_key,
        generated: false,
        auto_increment: false,
        read_only: false,
        filter: false,
        sort: false,
        unique: false,
    }
}

fn descriptor(
    database: OwnedResourceDatabase,
    owner_field: Option<&str>,
    owner_type: &str,
) -> OwnedResourceDescriptor {
    let (table, primary_key, fields) = match database {
        OwnedResourceDatabase::Tenant => (
            "biz_order",
            vec!["tenant_id".into(), "id".into()],
            vec![
                field("tenant_id", "String", true),
                field("id", "i64", true),
                field("owner_id", owner_type, false),
            ],
        ),
        OwnedResourceDatabase::Control => (
            "biz_global_order",
            vec!["id".into()],
            vec![
                field("id", "i64", true),
                field("owner_id", owner_type, false),
            ],
        ),
    };
    OwnedResourceDescriptor {
        name: "order".into(),
        title: "订单".into(),
        table: table.into(),
        collation: None,
        database,
        model_module: "order_business::resources".into(),
        model_type: "Order".into(),
        route: None,
        frontend_page: None,
        owner_field: owner_field.map(str::to_owned),
        menu_parent: None,
        menu_order: 100,
        primary_key,
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        fields,
    }
}

#[test]
fn owner_field_defaults_to_none_and_is_omitted_from_descriptor_json() {
    let descriptor = descriptor(OwnedResourceDatabase::Tenant, None, "i64");
    let resource = descriptor_to_ir(&package(), &descriptor).unwrap();
    assert!(resource.access.owner_field.is_none());
    let json = serde_json::to_value(&descriptor).unwrap();
    assert!(json.get("owner_field").is_none());
}

#[test]
fn declared_owner_field_flows_to_tenant_and_control_resources() {
    for database in [
        OwnedResourceDatabase::Tenant,
        OwnedResourceDatabase::Control,
    ] {
        for owner_type in ["i64", "Option<i64>"] {
            let descriptor = descriptor(database, Some("owner_id"), owner_type);
            let resource = descriptor_to_ir(&package(), &descriptor).unwrap();
            assert_eq!(resource.access.owner_field.as_deref(), Some("owner_id"));
            let owner = resource
                .fields
                .iter()
                .find(|field| field.name == "owner_id")
                .unwrap();
            assert!(!owner.usage.create && !owner.usage.create_optional && !owner.usage.update);
            assert!(matches!(owner.widget, crate::resource::WidgetIr::Hidden));
            assert!(owner.usage.read && owner.usage.list);
            assert_eq!(
                resource.access.system_tenant_only,
                database == OwnedResourceDatabase::Control
            );
        }
    }
}

#[test]
fn owner_field_rejects_empty_missing_and_non_i64_fields() {
    for (owner_field, owner_type, message) in [
        (Some(""), "i64", "owner_field 不能为空"),
        (Some(" owner_id"), "i64", "含有首尾空白"),
        (Some("missing"), "i64", "未在模型中声明"),
        (Some("owner_id"), "String", "必须是 i64 或 Option<i64> 字段"),
    ] {
        let error = descriptor_to_ir(
            &package(),
            &descriptor(OwnedResourceDatabase::Tenant, owner_field, owner_type),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains(message),
            "{owner_field:?} {owner_type}: {error}"
        );
    }
}

#[test]
fn resource_model_types_map_to_supported_form_widgets() {
    let mut descriptor = descriptor(OwnedResourceDatabase::Tenant, None, "i64");
    descriptor.fields = vec![
        field("tenant_id", "String", true),
        field("id", "i64", true),
        field("enabled", "bool", false),
        field("due_on", "ryframe_sdk::time::Date", false),
        field(
            "happens_at",
            "ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>",
            false,
        ),
        field("amount", "ryframe_sdk::rust_decimal::Decimal", false),
        field("metadata", "ryframe_sdk::serde_json::Value", false),
        field("note", "String", false),
        field("count", "i32", false),
        field("large_count", "i64", false),
    ];
    descriptor
        .fields
        .iter_mut()
        .find(|field| field.name == "amount")
        .unwrap()
        .sql_type = Some("DECIMAL(20,6)".into());
    let resource = descriptor_to_ir(&package(), &descriptor).unwrap();
    let widget = |name: &str| {
        resource
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap()
            .widget
    };
    assert_eq!(widget("enabled"), crate::resource::WidgetIr::Switch);
    assert_eq!(widget("due_on"), crate::resource::WidgetIr::Date);
    assert_eq!(widget("happens_at"), crate::resource::WidgetIr::DateTime);
    assert_eq!(widget("amount"), crate::resource::WidgetIr::Text);
    assert_eq!(widget("metadata"), crate::resource::WidgetIr::Json);
    assert_eq!(widget("note"), crate::resource::WidgetIr::Text);
    assert_eq!(widget("count"), crate::resource::WidgetIr::Number);
    assert_eq!(widget("large_count"), crate::resource::WidgetIr::Text);
}
