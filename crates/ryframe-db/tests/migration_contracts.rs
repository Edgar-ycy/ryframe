use std::collections::BTreeSet;

use ryframe_db::{
    install_id_generator,
    migration::{
        CONTROL_MIGRATION_LEDGER, Migrator, access_menus, access_permission_codes,
        access_permission_names, control_ddl_statements, expected_extra, extract_column_type,
        mysql_snapshot_sql, normalize_column_type, schema_fingerprint, supports_mysql_80_or_newer,
        validate_seed_statements,
    },
    next_id,
    resource_ownership::{marker, validate_marker_input},
};
use ryframe_kernel::AppResult;
use sea_orm_migration::MigratorTrait;

fn fixed_id() -> AppResult<i64> {
    Ok(42)
}

#[test]
fn installed_generator_is_used_and_cannot_be_replaced() {
    install_id_generator(fixed_id).expect("首次安装应成功");
    assert_eq!(next_id().expect("ID 应生成成功"), 42);
    assert!(install_id_generator(fixed_id).is_err());
}

#[test]
fn ownership_marker_is_stable_and_inputs_are_bounded() {
    assert_eq!(
        marker("test-a", "tenant-data"),
        "ryframe-owner:v1:test-a:tenant-data"
    );
    assert!(validate_marker_input("test-a", "control").is_ok());
    assert!(validate_marker_input("Test", "control").is_err());
    assert!(validate_marker_input("test", "tenant_data").is_err());
}

#[test]
fn access_catalog_seed_is_complete_and_unambiguous() {
    let permissions = access_permission_codes().expect("访问目录权限应可解析");
    let permission_set = permissions.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(permission_set.len(), permissions.len());
    assert!(permissions.iter().all(|code| code.len() <= 64));
    assert!(permission_set.contains("tenant:capability:override"));
    let permission_names = access_permission_names().expect("权限中文名称应可解析");
    assert_eq!(
        permission_names.get("tenant:data-migration:list"),
        Some(&"租户数据迁移查询")
    );
    assert!(
        permission_names
            .keys()
            .all(|code| permission_set.contains(code))
    );
    assert!(permission_names.values().all(|name| !name.is_empty()));

    let menus = access_menus().expect("访问目录菜单应可解析");
    let mut preceding_routes = BTreeSet::new();
    for menu in &menus {
        assert!(menu.route_key.len() <= 64);
        assert!(!menu.name.is_empty());
        assert!(menu.name.chars().count() <= 64);
        if let Some(parent) = menu.parent_route_key() {
            assert!(preceding_routes.contains(parent));
        }
        preceding_routes.insert(menu.route_key);
    }
    let route_keys = menus
        .iter()
        .map(|menu| menu.route_key)
        .collect::<BTreeSet<_>>();
    assert_eq!(route_keys.len(), menus.len());
    for menu in menus {
        if let Some(permission) = menu.permission {
            assert!(permission_set.contains(permission));
        }
    }
}

#[test]
fn review_snapshot_matches_the_fresh_schema() {
    let snapshot = mysql_snapshot_sql();
    assert!(snapshot.contains("schema fingerprint: 595a420d869c5fdb"));
    assert_eq!(snapshot.matches("CREATE TABLE IF NOT EXISTS").count(), 51);
    for required in [
        "`sys_background_job`",
        "`payload_version`",
        "`sys_export_job`",
        "`active_request_fingerprint`",
        "`delete_pending_at`",
    ] {
        assert!(snapshot.contains(required));
    }
}

#[test]
fn canonical_seed_statements_are_strictly_parseable() {
    validate_seed_statements().expect("基线种子应可严格解析");
}

#[test]
fn expected_column_type_keeps_unsigned_modifier() {
    assert_eq!(
        normalize_column_type(extract_column_type("SMALLINT UNSIGNED NOT NULL")),
        "smallintunsigned"
    );
    assert_eq!(
        normalize_column_type(extract_column_type("VARCHAR(64) CHARACTER SET utf8mb4")),
        "varchar(64)"
    );
}

#[test]
fn expected_extra_keeps_timestamp_precision() {
    assert_eq!(
        expected_extra("DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)"),
        "on update current_timestamp(6)"
    );
    assert_eq!(
        expected_extra("TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP"),
        "on update current_timestamp"
    );
}

#[test]
fn supported_version_rejects_old_mysql_mariadb_and_invalid_identity() {
    assert!(supports_mysql_80_or_newer(
        "8.0.16",
        "MySQL Community Server"
    ));
    assert!(supports_mysql_80_or_newer("9.1.0-commercial", "MySQL"));
    assert!(!supports_mysql_80_or_newer("8.0.15", "MySQL"));
    assert!(!supports_mysql_80_or_newer(
        "11.4.2-MariaDB",
        "MariaDB Server"
    ));
    assert!(!supports_mysql_80_or_newer("unknown", "MySQL"));
}

#[test]
fn control_schema_is_one_fresh_baseline() {
    let migrations = Migrator::migrations();
    assert_eq!(migrations.len(), 1);
    assert_eq!(CONTROL_MIGRATION_LEDGER, "seaql_migrations");
    assert_eq!(migrations[0].name(), "m20260820_000000_control_baseline");
}

#[test]
fn baseline_contains_export_snapshot_and_task_versions() {
    let statements = control_ddl_statements().collect::<Vec<_>>();
    let export = statements
        .iter()
        .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_export_job`"))
        .expect("基线必须包含导出表");
    let background = statements
        .iter()
        .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_background_job`"))
        .expect("基线必须包含后台任务表");
    for column in [
        "request_version",
        "authorization_fingerprint",
        "request_fingerprint",
        "active_request_fingerprint",
        "snapshot_at",
        "upper_id",
        "matched_rows",
        "exported_rows",
        "delete_pending_at",
    ] {
        assert!(export.contains(&format!("`{column}`")));
    }
    assert!(background.contains("`payload_version`"));
}

#[test]
fn baseline_table_set_and_schema_fingerprint_are_stable() {
    let mut tables = control_ddl_statements()
        .map(|statement| statement.split('`').nth(1).expect("基线语句必须包含表名"))
        .collect::<Vec<_>>();
    let count = tables.len();
    tables.sort_unstable();
    tables.dedup();
    assert_eq!(tables.len(), count);
    assert_eq!(count, 51);
    assert_eq!(schema_fingerprint(), "595a420d869c5fdb");
}
