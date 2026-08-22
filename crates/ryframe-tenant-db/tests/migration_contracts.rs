use ryframe_application::ports::tenant_data::TenantDataRow;
use ryframe_tenant_db::{
    application_ports::tenant_data::{
        business_cursor_columns, catalog_table, cursor_from_last_row, validate_batch_size,
    },
    migration::{
        Migrator, RESOURCE_OWNERSHIP_DDL, TENANT_DATA_CATALOG, TENANT_DATA_MIGRATION_LEDGER,
        TENANT_DATA_SCHEMA_FINGERPRINT, TENANT_FENCE_DDL, TENANT_TARGET_SLOT_DDL,
        TenantDataTableDescriptor, ensure_local_foreign_key_schema, normalize_check_clause,
    },
};
use sea_orm_migration::MigratorTrait;

#[test]
fn normalizes_mysql_escaped_check_literal_quotes() {
    assert_eq!(
        normalize_check_clause(r#"(`state` IN (_utf8mb4\'active\', _utf8mb4\'frozen\'))"#),
        "statein('active','frozen')",
    );
    assert_eq!(
        normalize_check_clause(r#"(`kind` = _ascii\"mysql\")"#),
        "kind=\"mysql\"",
    );
    assert_eq!(
        normalize_check_clause(r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#),
        normalize_check_clause("`code` = 'O''Reilly'"),
    );
    assert_ne!(
        normalize_check_clause(r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#),
        normalize_check_clause("`code` = 'OReilly'"),
    );
}

#[test]
fn check_normalization_preserves_semantic_grouping() {
    assert_eq!(normalize_check_clause("(((`a` = 1)))"), "a=1");
    assert_ne!(
        normalize_check_clause("((`a` AND `b`) OR `c`)"),
        normalize_check_clause("(`a` AND (`b` OR `c`))"),
    );
}

#[test]
fn check_normalization_preserves_literal_bytes_and_charset_like_text() {
    assert_ne!(
        normalize_check_clause("`code` = 'A'"),
        normalize_check_clause("`code` = 'a'"),
    );
    assert_ne!(
        normalize_check_clause("`label` = 'a b'"),
        normalize_check_clause("`label` = 'ab'"),
    );
    assert_ne!(
        normalize_check_clause("`code` = '_utf8mb4active'"),
        normalize_check_clause("`code` = 'active'"),
    );
    assert_ne!(
        normalize_check_clause("`code` = 'A` B'"),
        normalize_check_clause("`code` = 'a b'"),
    );
}

#[test]
fn cross_schema_foreign_keys_are_rejected() {
    assert!(ensure_local_foreign_key_schema("tenant_data", "control").is_err());
    assert!(ensure_local_foreign_key_schema("tenant_data", "tenant_data").is_ok());
}

#[test]
fn tenant_data_uses_one_fresh_baseline_and_its_own_ledger() {
    let migrations = Migrator::migrations();
    assert_eq!(migrations.len(), 1);
    assert_eq!(TENANT_DATA_MIGRATION_LEDGER, "seaql_tenant_data_migrations");
    assert_eq!(migrations[0].name(), "m20260820_000000_tenant_baseline");
}

#[test]
fn baseline_contains_complete_infrastructure_schema() {
    assert!(TENANT_FENCE_DDL.contains("ck_biz_tenant_fence_generation"));
    assert!(TENANT_FENCE_DDL.contains("ck_biz_tenant_fence_state"));
    assert!(TENANT_TARGET_SLOT_DDL.contains("ck_biz_tenant_target_slot_value"));
    assert!(RESOURCE_OWNERSHIP_DDL.contains("uq_resource_ownership_marker"));
}

#[test]
fn generated_fingerprint_matches_shared_catalog_computation() {
    assert_eq!(
        TENANT_DATA_CATALOG.schema_fingerprint(),
        TENANT_DATA_SCHEMA_FINGERPRINT
    );
}

#[test]
fn catalog_lookup_rejects_unknown_table() {
    assert!(catalog_table("unknown_table").is_err());
}

#[test]
fn batch_size_is_bounded() {
    assert!(validate_batch_size(1).is_ok());
    assert!(validate_batch_size(10_000).is_ok());
    assert!(validate_batch_size(0).is_err());
    assert!(validate_batch_size(10_001).is_err());
}

#[test]
fn cursor_is_derived_from_catalog_columns() {
    const COLUMNS: &[&str] = &["tenant_id", "id", "name"];
    const CURSOR_COLUMNS: &[&str] = &["tenant_id", "id"];
    const COLUMN_TYPES: &[&str] = &["varchar", "bigint", "varchar"];
    const DESCRIPTOR: TenantDataTableDescriptor = TenantDataTableDescriptor {
        table: "biz_example",
        copy_order: 1,
        tenant_column: "tenant_id",
        primary_key_cursor_columns: CURSOR_COLUMNS,
        checksum_columns: COLUMNS,
        column_types: COLUMN_TYPES,
        has_generated_columns: false,
        foreign_key_dependencies: &[],
        foreign_keys: &[],
        schema_canonical: "test",
    };

    let cursor_columns = business_cursor_columns(&DESCRIPTOR);
    let row = DESCRIPTOR
        .checksum_columns
        .iter()
        .map(|column| Some((*column).to_owned()))
        .collect::<TenantDataRow>();
    let cursor = cursor_from_last_row(&[row], &DESCRIPTOR, &cursor_columns)
        .expect("应按 catalog 生成 cursor");
    assert_eq!(
        cursor,
        cursor_columns
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>()
    );
}
