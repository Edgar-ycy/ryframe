use std::fs;

use ryframe_generator::command::{GenerationBuilder, GenerationOperation};

#[test]
fn configuration_and_builder_share_resolved_tasks_without_executing_them() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::create_dir(root.path().join(".local-tests")).unwrap();
    fs::write(root.path().join("config/tools.toml"), "[generator]\npackage='old-business'\nsource='sources.legacy'\nstorage='control'\ntables=['old']\nsync_frontend=true\n").unwrap();
    fs::write(root.path().join(".local-tests/tools.toml"), "[generator]\npackage='order-business'\ntables=['biz_order','biz_order_item']\nstrip_table_prefix='biz_'\n").unwrap();
    let configured = GenerationBuilder::new(root.path()).plan().unwrap();
    let explicit = GenerationBuilder::new(root.path())
        .package("order-business")
        .source("sources.legacy")
        .database("control")
        .include_tables(["biz_order"])
        .include_tables(["biz_order_item"])
        .strip_table_prefix("biz_")
        .plan()
        .unwrap();
    assert_eq!(configured.operation, GenerationOperation::Import);
    assert_eq!(configured.tables, explicit.tables);
    assert_eq!(configured.tasks(), explicit.tasks());
    assert_eq!(configured.describe(), explicit.describe());
    assert!(!root.path().join("target").exists());
    assert!(!root.path().join("Cargo.lock").exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn explicit_empty_selection_and_disabled_sync_override_configuration() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/tools.toml"), "[generator]\npackage='order-business'\ntables=['biz_order']\nmodels=['Old']\nsync_frontend=true\n").unwrap();
    let plan = GenerationBuilder::new(root.path())
        .include_tables(Vec::<String>::new())
        .include_models(["Order", "OrderItem"])
        .sync_frontend(false)
        .write(true)
        .plan()
        .unwrap();
    assert_eq!(plan.operation, GenerationOperation::Resource);
    assert_eq!(plan.models, ["Order", "OrderItem"]);
    assert!(!plan.sync_frontend);
    assert_eq!(plan.tasks().last(), Some(&"事务写入文件"));
}

#[test]
fn new_business_uses_the_requested_package_for_cli_and_builder_names() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/tools.toml"),
        "[generator]\npackage='other-business'\ntables=['old']\n",
    )
    .unwrap();
    let plans = ["sales-orders", "sales-orders-business"].map(|name| {
        GenerationBuilder::new(root.path())
            .operation(GenerationOperation::NewBusiness(name.into()))
            .package("ignored-business")
            .plan()
            .unwrap()
    });
    assert_eq!(
        plans[0].operation,
        GenerationOperation::NewBusiness("sales-orders".into())
    );
    assert_eq!(plans[0].package, "sales-orders-business");
    assert_eq!(plans[0].describe(), plans[1].describe());
    assert!(!plans[0].describe().contains("other-business"));
    assert!(!plans[0].describe().contains("只读结构来源"));
    let repeated = GenerationBuilder::new(root.path())
        .operation(GenerationOperation::NewBusiness(
            "order-business-business".into(),
        ))
        .plan()
        .unwrap();
    assert_eq!(repeated.package, "order-business-business");
    assert_eq!(
        repeated.operation,
        GenerationOperation::NewBusiness("order-business".into())
    );
    assert!(!root.path().join(".local-tests").exists());
    assert!(!root.path().join("target").exists());
}

#[test]
fn invalid_batch_names_and_target_fail_before_execution() {
    let root = tempfile::tempdir().unwrap();
    for tables in [
        vec!["biz_order", "BIZ_ORDER"],
        vec!["biz_order", "../outside"],
        vec!["biz_order", ""],
    ] {
        assert!(
            GenerationBuilder::new(root.path())
                .package("order-business")
                .database("control")
                .include_tables(tables)
                .plan()
                .is_err()
        );
    }
    assert!(
        GenerationBuilder::new(root.path())
            .package("order-business")
            .database("other")
            .include_tables(["biz_order"])
            .plan()
            .is_err()
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
