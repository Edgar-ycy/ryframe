use std::{fs, path::Path, process::Command};

use ryframe_generator::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package,
};
use ryframe_generator::{
    command::{GenerationBuilder, GenerationOperation},
    import::{ColumnInfo, TableInfo, plan_import_models},
};

pub(super) fn verify(backend: &Path, target: &Path) {
    bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: backend,
        module: "invoice",
        write: true,
    })
    .expect("真实 Workspace 应创建业务 crate 并装配进程");
    fs::write(
        backend.join("crates/invoice-business/src/resources/mod.rs"),
        MODEL,
    )
    .expect("应写入租户自增主键业务模型");
    let metadata = Command::new("cargo")
        .args(["metadata", "--offline", "--format-version", "1"])
        .current_dir(backend)
        .output()
        .unwrap();
    super::assert_command_succeeded("显式登记业务依赖锁文件", &metadata);
    let lock = fs::read(backend.join("Cargo.lock")).unwrap();
    let report = generate_business_package(BusinessGenerateOptions {
        current_dir: backend,
        package: "invoice-business",
        models: &[],
        write: true,
    })
    .expect("应生成真实业务资源");
    assert!(!report.created.is_empty());
    verify_selected_generation(backend);
    import_batch(backend);
    verify_incremental_generation(backend);
    let repeat = generate_business_package(BusinessGenerateOptions {
        current_dir: backend,
        package: "invoice-business",
        models: &[],
        write: true,
    })
    .expect("业务资源重复生成应零差异");
    assert!(repeat.created.is_empty() && repeat.updated.is_empty() && repeat.removed.is_empty());
    assert_eq!(fs::read(backend.join("Cargo.lock")).unwrap(), lock);
    for features in ["catalog", "migration", "api"] {
        let output = Command::new("cargo")
            .args([
                "clippy",
                "--locked",
                "--offline",
                "--no-default-features",
                "-p",
                "invoice-business",
                "--features",
                features,
                "--jobs",
                "4",
                "--",
                "-D",
                "warnings",
                "-D",
                "clippy::redundant_clone",
            ])
            .current_dir(backend)
            .env("CARGO_TARGET_DIR", target)
            .output()
            .unwrap();
        super::assert_command_succeeded(&format!("生成业务 {features} 编译"), &output);
    }
    let formatted = Command::new("cargo")
        .args(["fmt", "--all", "--", "--check"])
        .current_dir(backend)
        .output()
        .unwrap();
    super::assert_command_succeeded("业务 crate 与装配代码格式", &formatted);
    let output = Command::new("cargo")
        .args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "invoice-business",
            "--no-default-features",
            "--features",
            "api",
            "--test",
            "nullable_contract",
            "--jobs",
            "4",
        ])
        .current_dir(backend)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap();
    super::assert_command_succeeded("业务可空字段的真实 DTO 契约", &output);
}

fn verify_incremental_generation(backend: &Path) {
    let root = backend.join("crates/invoice-business");
    let model = root.join("src/resources/mod.rs");
    let initial = root.join("migrations/m_resource_initial_global_item.rs");
    let bytes = fs::read(&initial).unwrap();
    fs::write(&model, fs::read_to_string(&model).unwrap().replace("    pub code: String,", "    pub code: String,\n    #[resource(sql_type = \"TEXT\")]\n    pub memo: Option<String>,\n    #[resource(default = \"0\")]\n    pub amount: i32,")).unwrap();
    GenerationBuilder::new(backend)
        .operation(GenerationOperation::Resource)
        .package("invoice-business")
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .expect("模型增量生成应追加控制库及租户库迁移");
    assert_eq!(fs::read(&initial).unwrap(), bytes);
    assert_eq!(fs::read_dir(root.join("migrations")).unwrap().count(), 8);
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/nullable_contract.rs"), r#"use invoice_business::generated::dto::global_item::{CreateGlobalItemDto, UpdateGlobalItemDto};
use ryframe_sdk::serde_json::{self, json};

#[test]
fn nullable_values_follow_complete_replacement_contract() {
    let missing: CreateGlobalItemDto = serde_json::from_value(json!({"code":"first"})).unwrap();
    assert!(missing.memo.is_none());
    let empty: UpdateGlobalItemDto = serde_json::from_value(json!({"memo":null})).unwrap();
    assert!(empty.memo.is_none());
    let value: UpdateGlobalItemDto = serde_json::from_value(json!({"memo":"text"})).unwrap();
    assert_eq!(value.memo.as_deref(), Some("text"));
    let omitted: UpdateGlobalItemDto = serde_json::from_value(json!({})).unwrap();
    assert!(omitted.memo.is_none());
}
"#).unwrap();
    let formatted = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(root.join("tests/nullable_contract.rs"))
        .current_dir(backend)
        .output()
        .unwrap();
    super::assert_command_succeeded("业务 DTO 测试夹具格式", &formatted);
}

fn verify_selected_generation(backend: &Path) {
    let root = backend.join("crates/invoice-business");
    let ownership = fs::read(root.join(".ryframe/generated.toml")).unwrap();
    GenerationBuilder::new(backend)
        .operation(GenerationOperation::Resource)
        .package("invoice-business")
        .include_models(["Invoice"])
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .expect("未选中模型的迁移注册必须保留且零差异");
    assert_eq!(
        fs::read(root.join(".ryframe/generated.toml")).unwrap(),
        ownership
    );
    let model = root.join("src/resources/mod.rs");
    let original = fs::read_to_string(&model).unwrap();
    fs::write(&model, original.replace("全局字典", "全局改名")).unwrap();
    let error = GenerationBuilder::new(backend)
        .operation(GenerationOperation::Resource)
        .package("invoice-business")
        .include_models(["Invoice"])
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .unwrap_err();
    assert!(error.contains("未选中的资源 global_item"));
    assert_eq!(
        fs::read(root.join(".ryframe/generated.toml")).unwrap(),
        ownership
    );
    fs::write(&model, original).unwrap();
}

fn import_batch(backend: &Path) {
    let root = backend.join("crates/invoice-business");
    let mut minimal = import_table("biz_minimal");
    minimal
        .columns
        .retain(|column| !matches!(column.name.as_str(), "created_at" | "updated_at"));
    let mut soft_minimal = minimal.clone();
    soft_minimal.table_name = "biz_soft_minimal".into();
    soft_minimal.schema_canonical = "fixture:biz_soft_minimal".into();
    let mut deleted = soft_minimal
        .columns
        .iter()
        .find(|column| column.name == "status")
        .unwrap()
        .clone();
    deleted.name = "del_flag".into();
    soft_minimal.columns.push(deleted);
    let tables = [
        import_table("biz_memo"),
        import_table("biz_memo_item"),
        minimal,
        soft_minimal,
    ];
    plan_import_models(backend, &root, &tables, "control", Some("biz_"))
        .unwrap()
        .write()
        .unwrap();
    let numeric_model = root.join("src/resources/memo_item.rs");
    let source = fs::read_to_string(&numeric_model).unwrap();
    fs::write(
        &numeric_model,
        source.replace(
            "    pub status: i32,",
            "    #[resource(filter)]\n    pub status: i32,",
        ),
    )
    .unwrap();
    GenerationBuilder::new(backend)
        .operation(GenerationOperation::Resource)
        .package("invoice-business")
        .include_models(["Memo", "MemoItem", "Minimal", "SoftMinimal"])
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .expect("导入注册后的模型及无审计字段表必须生成可编译后端 CRUD");
    for name in ["memo", "memo_item", "minimal", "soft_minimal"] {
        assert!(
            root.join(format!("src/generated/repositories/{name}.rs"))
                .is_file()
        );
        assert!(
            root.join(format!("src/generated/handlers/{name}.rs"))
                .is_file()
        );
    }
}

fn import_table(name: &str) -> TableInfo {
    let columns = [
        ("id", "i64", "bigint"),
        ("code", "String", "varchar(255)"),
        ("status", "i32", "int"),
        (
            "created_at",
            "ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>",
            "datetime(6)",
        ),
        (
            "updated_at",
            "ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>",
            "datetime(6)",
        ),
    ]
    .into_iter()
    .map(|(name, rust_type, column_type)| ColumnInfo {
        name: name.into(),
        data_type: column_type.split('(').next().unwrap().into(),
        column_type: column_type.into(),
        rust_type: rust_type.into(),
        default: None,
        extra: if name == "id" {
            "auto_increment".into()
        } else {
            String::new()
        },
        is_nullable: false,
        is_primary_key: name == "id",
        is_unique: false,
        is_auto_increment: name == "id",
        comment: None,
    })
    .collect();
    TableInfo {
        table_name: name.into(),
        comment: None,
        columns,
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        foreign_key_dependencies: Vec::new(),
        schema_canonical: format!("fixture:{name}"),
    }
}

const MODEL: &str = r#"#[derive(ryframe_sdk::ResourceModel)]
#[resource(
    name = "invoice",
    title = "发票",
    table = "biz_invoice",
    database = "tenant",
    primary_key = ["tenant_id", "id"]
)]
pub struct Invoice {
    #[resource(primary_key, auto_increment, column = "invoice_id")]
    pub id: i64,
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(unique, filter, sort, sql_type = "VARCHAR(32)")]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}

pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![
        <Invoice as ryframe_sdk::ResourceModel>::descriptor(),
        <GlobalItem as ryframe_sdk::ResourceModel>::descriptor(),
    ]
}

#[derive(ryframe_sdk::ResourceModel)]
#[resource(
    name = "global_item",
    title = "全局字典",
    table = "biz_global_item",
    database = "control"
)]
pub struct GlobalItem {
    #[resource(primary_key, auto_increment)]
    pub id: i64,
    #[resource(unique, filter, sort, sql_type = "VARCHAR(32)")]
    pub code: String,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}
"#;
