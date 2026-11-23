use std::{fs, path::Path, process::Command};

use ryframe_generator::business::{
    BusinessBootstrapOptions, BusinessGenerateOptions, bootstrap_business_package,
    generate_business_package,
};
use ryframe_generator::{
    command::{GenerationBuilder, GenerationOperation},
    import::{ColumnInfo, IndexInfo, TableInfo, plan_import_models},
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
        catalog_target: None,
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
        catalog_target: None,
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
            "--test",
            "decimal_contract",
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
    fs::write(&model, fs::read_to_string(&model).unwrap().replace("    pub code: String,", "    pub code: String,\n    #[resource(sql_type = \"TEXT\")]\n    pub memo: Option<String>,\n    #[resource(default = \"0\", filter)]\n    pub amount: i32,")).unwrap();
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
    fs::write(
        &model,
        fs::read_to_string(&model)
            .unwrap()
            .replace("pub amount: i32", "pub amount: i64"),
    )
    .unwrap();
    GenerationBuilder::new(backend)
        .operation(GenerationOperation::Resource)
        .package("invoice-business")
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .expect("真实业务模型须完成 i32 到 i64 的第二次扩宽，再验证精确字符串 DTO");
    assert_eq!(fs::read(&initial).unwrap(), bytes);
    assert_eq!(fs::read_dir(root.join("migrations")).unwrap().count(), 10);
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tests/nullable_contract.rs"),
        include_str!("../fixtures/business_dto_contract.rs"),
    )
    .unwrap();
    fs::write(
        root.join("tests/decimal_contract.rs"),
        include_str!("../fixtures/business_decimal_contract.rs"),
    )
    .unwrap();
    let formatted = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(root.join("tests/nullable_contract.rs"))
        .arg(root.join("tests/decimal_contract.rs"))
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
    let unsigned = minimal
        .columns
        .iter_mut()
        .find(|column| column.name == "id")
        .unwrap();
    unsigned.rust_type = "u64".into();
    unsigned.column_type = "bigint unsigned".into();
    unsigned.is_auto_increment = false;
    unsigned.extra.clear();
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
    let mut memo_item = import_table("biz_memo_item");
    memo_item
        .foreign_keys
        .push(ryframe_generator::import::ForeignKeyInfo {
            name: "fk_memo_item_memo".into(),
            columns: vec!["status".into()],
            referenced_table: "biz_memo".into(),
            referenced_columns: vec!["status".into()],
            update_rule: ryframe_generator::import::ForeignKeyAction::Cascade,
            delete_rule: ryframe_generator::import::ForeignKeyAction::Restrict,
            match_option: ryframe_generator::import::ForeignKeyMatch::None,
        });
    let mut memo = import_table("biz_memo");
    memo.indexes.push(IndexInfo {
        name: "uk_memo_status".into(),
        columns: vec!["status".into()],
        unique: true,
        index_type: "BTREE".into(),
    });
    let tables = [memo, memo_item, minimal, soft_minimal];
    plan_import_models(backend, &root, &tables, "control", Some("biz_"))
        .unwrap()
        .write()
        .unwrap();
    let numeric_model = root.join("src/resources/memo_item.rs");
    let source = fs::read_to_string(&numeric_model).unwrap();
    fs::write(
        &numeric_model,
        source
            .replace(
                "    pub status: i32,",
                "    #[resource(filter)]\n    pub status: i32,",
            )
            .replace(
                "    pub large_count: u64,",
                "    #[resource(filter)]\n    pub large_count: u64,",
            )
            .replace("pub id: i64", "pub id: u64")
            .replace(
                "    pub price: ryframe_sdk::rust_decimal::Decimal,",
                "    #[resource(filter)]\n    pub price: ryframe_sdk::rust_decimal::Decimal,",
            )
            .replace("sql_type = \"bigint\"", "sql_type = \"bigint unsigned\""),
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
        ("tiny_count", "i8", "tinyint"),
        ("small_count", "i16", "smallint"),
        ("byte_count", "u8", "tinyint unsigned"),
        ("short_count", "u16", "smallint unsigned"),
        ("count", "u32", "int unsigned"),
        ("large_count", "u64", "bigint unsigned"),
        (
            "price",
            "ryframe_sdk::rust_decimal::Decimal",
            "decimal(20,6)",
        ),
        (
            "discount",
            "Option<ryframe_sdk::rust_decimal::Decimal>",
            "decimal(28,6)",
        ),
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
        data_type: column_type.split(['(', ' ']).next().unwrap().into(),
        column_type: column_type.into(),
        collation: (rust_type == "String").then(|| "utf8mb4_bin".into()),
        rust_type: rust_type.into(),
        default: match name {
            "created_at" | "updated_at" => Some("CURRENT_TIMESTAMP(6)".into()),
            "large_count" => Some(u64::MAX.to_string()),
            "price" => Some("0.125000".into()),
            _ => None,
        },
        extra: if name == "id" {
            "auto_increment".into()
        } else if name == "created_at" {
            "DEFAULT_GENERATED".into()
        } else if name == "updated_at" {
            "DEFAULT_GENERATED on update CURRENT_TIMESTAMP(6)".into()
        } else {
            String::new()
        },
        is_nullable: name == "discount",
        is_primary_key: name == "id",
        is_unique: false,
        is_auto_increment: name == "id",
        comment: (name == "price").then(|| "金额 '原样'\\保存".into()),
    })
    .collect();
    TableInfo {
        table_name: name.into(),
        collation: Some("utf8mb4_0900_ai_ci".into()),
        comment: None,
        columns,
        indexes: vec![IndexInfo {
            name: format!("idx_{name}_status_code"),
            columns: vec!["status".into(), "code".into()],
            unique: false,
            index_type: "BTREE".into(),
        }],
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
    primary_key = ["tenant_id", "id"],
    owner_field = "owner_id"
)]
pub struct Invoice {
    #[resource(primary_key, auto_increment, column = "invoice_id")]
    pub id: i64,
    #[resource(primary_key)]
    pub tenant_id: String,
    #[resource(unique, filter, sort, sql_type = "VARCHAR(32)")]
    pub code: String,
    #[resource(filter)]
    pub owner_id: i64,
    #[resource(filter)]
    pub starts_on: ryframe_sdk::time::Date,
    pub ends_on: Option<ryframe_sdk::time::Date>,
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
    pub payload: Option<ryframe_sdk::serde_json::Value>,
    #[resource(read_only)]
    pub created_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
    #[resource(read_only)]
    pub updated_at: ryframe_sdk::chrono::DateTime<ryframe_sdk::chrono::Utc>,
}
"#;
