use std::fs;

use ryframe_generator::import::{ColumnInfo, TableInfo, plan_import_models};

fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src/resources")).unwrap();
    fs::write(root.path().join("src/resources/mod.rs"),
        "// 保留中文说明和手写扩展\npub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> { vec![] }\nfn extension() {}\n").unwrap();
    root
}

fn table(name: &str) -> TableInfo {
    TableInfo {
        table_name: name.into(),
        comment: Some("订单\"换行\n说明".into()),
        columns: vec![ColumnInfo {
            name: "id".into(),
            data_type: "bigint".into(),
            column_type: "bigint".into(),
            default: None,
            extra: "auto_increment".into(),
            rust_type: "i64".into(),
            is_nullable: false,
            is_primary_key: true,
            is_unique: false,
            is_auto_increment: true,
            comment: None,
        }],
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        foreign_key_dependencies: Vec::new(),
        schema_canonical: format!("canonical-{name}"),
    }
}

#[test]
fn two_models_register_together_preserve_schema_and_repeat_without_writes() {
    let root = workspace();
    let tables = [table("biz_order"), table("biz_order_item")];
    let preview =
        plan_import_models(root.path(), root.path(), &tables, "control", Some("biz_")).unwrap();
    assert_eq!(preview.created.len(), 4);
    assert_eq!(preview.updated, ["src/resources/mod.rs"]);
    assert_eq!(preview.rust_sources().count(), 3);
    assert!(!root.path().join("src/resources/order.rs").exists());
    preview.write().unwrap();
    let registry = fs::read_to_string(root.path().join("src/resources/mod.rs")).unwrap();
    assert!(registry.contains("保留中文说明和手写扩展"));
    assert!(registry.contains("fn extension() {}"));
    assert!(registry.contains("pub mod order;") && registry.contains("pub mod order_item;"));
    assert!(registry.contains("order_item::OrderItem as ryframe_sdk::ResourceModel"));
    syn::parse_file(&registry).unwrap();
    let source = fs::read_to_string(root.path().join("src/resources/order.rs")).unwrap();
    assert!(source.contains("name = \"order\"") && source.contains("table = \"biz_order\""));
    assert!(source.contains("auto_increment"));
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join(".ryframe/imports/order.json")).unwrap())
            .unwrap();
    assert_eq!(schema["schema_canonical"], "canonical-biz_order");
    assert_eq!(schema["columns"][0]["column_type"], "bigint");
    let modified = fs::metadata(root.path().join("src/resources/mod.rs"))
        .unwrap()
        .modified()
        .unwrap();
    let repeat =
        plan_import_models(root.path(), root.path(), &tables, "control", Some("biz_")).unwrap();
    assert!(repeat.created.is_empty() && repeat.updated.is_empty());
    assert_eq!(repeat.unchanged.len(), 5);
    repeat.write().unwrap();
    assert_eq!(
        fs::metadata(root.path().join("src/resources/mod.rs"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn second_table_error_and_name_collision_leave_first_model_unwritten() {
    let root = workspace();
    let mut invalid = table("biz_item");
    invalid.columns[0].is_primary_key = false;
    assert!(
        plan_import_models(
            root.path(),
            root.path(),
            &[table("biz_order"), invalid],
            "control",
            Some("biz_")
        )
        .is_err()
    );
    assert!(!root.path().join("src/resources/order.rs").exists());
    assert!(
        plan_import_models(
            root.path(),
            root.path(),
            &[table("biz_order_item"), table("biz_order__item")],
            "control",
            Some("biz_")
        )
        .is_err()
    );
    fs::write(root.path().join("src/resources/item.rs"), "// 手写模型").unwrap();
    let error = plan_import_models(
        root.path(),
        root.path(),
        &[table("biz_order"), table("biz_item")],
        "control",
        Some("biz_"),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("已存在") && error.contains("本批未写入"));
    assert!(!root.path().join("src/resources/order.rs").exists());
    assert!(!root.path().join(".ryframe").exists());
}

#[test]
fn changed_registration_after_preview_cannot_be_overwritten() {
    let root = workspace();
    let plan = plan_import_models(
        root.path(),
        root.path(),
        &[table("biz_order"), table("biz_order_item")],
        "control",
        Some("biz_"),
    )
    .unwrap();
    fs::write(root.path().join("src/resources/mod.rs"), "// 并发手写变更").unwrap();
    assert!(
        plan.write()
            .unwrap_err()
            .to_string()
            .contains("compare-and-swap")
    );
    assert!(!root.path().join("src/resources/order.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("src/resources/mod.rs")).unwrap(),
        "// 并发手写变更"
    );
}
