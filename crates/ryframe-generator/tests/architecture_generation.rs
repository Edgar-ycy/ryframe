use ryframe_generator::{
    GenerateOptions, GeneratedFile, render_tables,
    schema::{ColumnInfo, IndexInfo, TableInfo},
    write_to_disk,
};

fn column(name: &str, data_type: &str, rust_type: &str, primary_key: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: data_type.into(),
        rust_type: rust_type.into(),
        is_nullable: rust_type.starts_with("Option<"),
        is_primary_key: primary_key,
        is_unique: false,
        is_auto_increment: false,
        comment: None,
    }
}

fn device_table() -> TableInfo {
    TableInfo {
        table_name: "biz_device".into(),
        comment: Some("设备".into()),
        columns: vec![
            column("tenant_id", "varchar", "String", true),
            column("id", "bigint", "i64", true),
            column("name", "varchar", "String", false),
            column("status", "tinyint", "i8", false),
            column("created_at", "datetime", "DateTime<Utc>", false),
            column(
                "updated_at",
                "datetime",
                "Option<DateTime<Utc>>",
                false,
            ),
            column("del_flag", "tinyint", "i8", false),
        ],
        indexes: vec![IndexInfo {
            name: "PRIMARY".into(),
            unique: true,
            index_type: "BTREE".into(),
            columns: vec!["tenant_id".into(), "id".into()],
        }],
        foreign_keys: Vec::new(),
        foreign_key_dependencies: Vec::new(),
        schema_canonical: "biz_device(tenant_id varchar,id bigint,name varchar,status tinyint,created_at datetime(6),updated_at datetime(6),del_flag tinyint)".into(),
    }
}

fn rendered_files() -> Vec<GeneratedFile> {
    let options = GenerateOptions {
        tables: vec!["biz_device".into()],
        table_prefixes: vec!["biz_".into()],
        ..GenerateOptions::default()
    };
    render_tables(&[device_table()], &options).expect("结构应可纯渲染")
}

fn content<'a>(files: &'a [GeneratedFile], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|file| file.path.ends_with(suffix))
        .map(|file| file.content.as_str())
        .unwrap_or_else(|| panic!("缺少生成文件：{suffix}"))
}

#[test]
fn generated_contract_matches_tracked_golden_files() {
    let files = rendered_files();
    assert_eq!(
        content(&files, "/device_dto.rs"),
        include_str!("golden/device_dto.golden")
    );
    assert_eq!(
        content(&files, "/device_use_case.rs"),
        include_str!("golden/device_use_case.golden")
    );
}

#[test]
fn generated_layers_follow_crate_and_transaction_boundaries() {
    let files = rendered_files();
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"crates/ryframe-db/src/repositories/business/device_repo.rs"));
    assert!(paths.contains(&"crates/ryframe-application/src/business/device_use_case.rs"));
    assert!(paths.contains(&"crates/ryframe-api/src/handlers/business/device_handler.rs"));
    assert!(paths.contains(&"crates/ryframe-api/src/dto/business/device_dto.rs"));

    let repository = content(&files, "/device_repo.rs");
    assert!(repository.contains("transaction: &DatabaseTransaction"));
    assert!(repository.contains("connection: &DatabaseConnection"));
    for forbidden in [".begin(", ".commit(", ".rollback(", "TransactionTrait"] {
        assert!(
            !repository.contains(forbidden),
            "Repository 不得包含 {forbidden}"
        );
    }

    let use_case = content(&files, "/device_use_case.rs");
    for expected in [
        "self.data_source.begin(tenant_id)",
        "self.data_source.commit(transaction)",
        "self.data_source.rollback(transaction)",
    ] {
        assert!(use_case.contains(expected), "应用用例缺少 {expected}");
    }
    for forbidden in [
        "ryframe_db",
        "ryframe_tenant_db",
        "ryframe_adapters",
        "ryframe_http",
        "sea_orm",
        "axum",
    ] {
        assert!(
            !use_case.contains(forbidden),
            "应用用例越界依赖 {forbidden}"
        );
    }

    let handler = content(&files, "/device_handler.rs");
    assert!(handler.contains("http::{ApiPageResponse, ApiResponse, HttpResult}"));
    assert!(!handler.contains("ryframe_http"));
    assert!(!handler.contains("ryframe_auth::RequestPrincipal"));
}

#[test]
fn every_generated_rust_file_has_valid_syntax() {
    for file in rendered_files() {
        if file.path.ends_with(".rs") {
            syn::parse_file(&file.content)
                .unwrap_or_else(|error| panic!("{} 语法无效：{error}", file.path));
        }
    }
}

#[tokio::test]
async fn disk_writer_skips_existing_files_without_overwrite() {
    let workspace = tempfile::tempdir().expect("应创建临时工作区");
    let initial = [GeneratedFile {
        path: "generated/device.rs".into(),
        content: "pub const VERSION: u8 = 1;\n".into(),
    }];
    let first = write_to_disk(&initial, workspace.path(), false)
        .await
        .expect("首次应写入");
    assert_eq!(first.written, ["generated/device.rs"]);

    let replacement = [GeneratedFile {
        path: "generated/device.rs".into(),
        content: "pub const VERSION: u8 = 2;\n".into(),
    }];
    let second = write_to_disk(&replacement, workspace.path(), false)
        .await
        .expect("已有文件应安全跳过");
    assert_eq!(second.skipped, ["generated/device.rs"]);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("generated/device.rs"))
            .expect("应读取首次内容"),
        initial[0].content
    );
}
