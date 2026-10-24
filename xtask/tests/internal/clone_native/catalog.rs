use std::{collections::BTreeMap, fs};

use super::{Fixture, native};

#[test]
fn generated_catalog_preserves_ddl_type_order_and_rejects_duplicate_columns() {
    let fixture = Fixture::new();
    assert_eq!(
        native::catalog::load(&fixture.root).unwrap().sources.len(),
        3
    );
    let generated = fixture.root.join("crates/ryframe-tenant-db/src/generated");
    let catalog = generated.join("catalog.rs");
    let descriptor = r#"TenantDataTableDescriptor {
        table: "biz_ordered",
        tenant_column: "tenant_id",
        column_types: &["varchar", "bigint"],
        has_generated_columns: false,
    }"#;
    let ddl =
        "CREATE TABLE `biz_ordered` (\n`tenant_id` VARCHAR(64),\n`id` BIGINT\n) ENGINE=InnoDB;";
    let migration = generated.join("business_ordered_migration.rs");
    fs::write(&catalog, descriptor).unwrap();
    fs::write(
        &migration,
        format!("pub const CREATE_TABLE_DDL: &str = r#\"{ddl}\"#;"),
    )
    .unwrap();
    let loaded = native::catalog::load(&fixture.root).unwrap();
    assert!(loaded.sources.contains(&migration));
    assert_eq!(loaded.sources.len(), 4);
    assert_eq!(
        loaded.tenant["biz_ordered"],
        BTreeMap::from([
            ("id".to_owned(), "BIGINT".to_owned()),
            ("tenant_id".to_owned(), "VARCHAR".to_owned()),
        ])
    );
    let source = loaded
        .sources
        .iter()
        .find(|path| **path == migration)
        .unwrap();
    let before = native::digest::file(source).unwrap();
    let changed = ddl.replace("VARCHAR(64)", "VARCHAR(128)");
    fs::write(
        &migration,
        format!("pub const CREATE_TABLE_DDL: &str = r#\"{changed}\"#;"),
    )
    .unwrap();
    assert!(native::catalog::load(&fixture.root).is_ok());
    assert_ne!(native::digest::file(source).unwrap(), before);
    let alternate = generated.join("ordered/migration.rs");
    fs::create_dir_all(alternate.parent().unwrap()).unwrap();
    fs::copy(&migration, &alternate).unwrap();
    assert!(
        native::catalog::load(&fixture.root)
            .err()
            .unwrap()
            .to_string()
            .contains("缺少同名迁移")
    );
    fs::remove_file(&alternate).unwrap();
    fs::write(
        &catalog,
        descriptor.replace("\"varchar\", \"bigint\"", "\"bigint\", \"varchar\""),
    )
    .unwrap();
    assert!(
        native::catalog::load(&fixture.root)
            .err()
            .unwrap()
            .to_string()
            .contains("迁移列类型不一致")
    );
    fs::write(&catalog, descriptor).unwrap();
    let duplicate = ddl.replace("`id` BIGINT", "`tenant_id` BIGINT");
    fs::write(
        &migration,
        format!("pub const CREATE_TABLE_DDL: &str = r#\"{duplicate}\"#;"),
    )
    .unwrap();
    assert!(
        native::catalog::load(&fixture.root)
            .err()
            .unwrap()
            .to_string()
            .contains("schema 表或列重复/缺失")
    );
    assert!(native::catalog::parse_schema(&format!("{ddl}\n{ddl}")).is_err());
    fs::remove_file(&migration).unwrap();
    assert!(
        native::catalog::load(&fixture.root)
            .err()
            .unwrap()
            .to_string()
            .contains("缺少同名迁移")
    );
}
