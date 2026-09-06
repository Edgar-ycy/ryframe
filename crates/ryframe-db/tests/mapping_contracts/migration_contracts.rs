use std::collections::BTreeSet;

use ryframe_db::{
    install_id_generator,
    migration::{
        CONTROL_MIGRATION_LEDGER, Migrator, access_menus, access_permission_codes,
        access_permission_names, control_ddl_statements, expected_extra, expected_migration_names,
        extract_column_type, mysql_snapshot_sql, normalize_column_type, schema_fingerprint,
        supports_mysql_80_or_newer, validate_seed_statements,
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
    let permission_set = permissions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(permission_set.len(), permissions.len());
    assert!(permissions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(permissions.iter().all(|code| code.len() <= 64));
    assert!(permission_set.contains("tenant:capability:override"));
    let permission_names = access_permission_names().expect("权限中文名称应可解析");
    assert_eq!(
        permission_names
            .get("tenant:data-migration:list")
            .map(String::as_str),
        Some("租户数据迁移查询")
    );
    assert!(
        permission_names
            .keys()
            .all(|code| permission_set.contains(code.as_str()))
    );
    assert!(permission_names.values().all(|name| !name.is_empty()));

    let menus = access_menus().expect("访问目录菜单应可解析");
    assert!(
        menus
            .windows(2)
            .all(|pair| pair[0].route_key < pair[1].route_key)
    );
    let mut preceding_routes = BTreeSet::new();
    for menu in &menus {
        assert!(menu.route_key.len() <= 64);
        assert!(!menu.name.is_empty());
        assert!(menu.name.chars().count() <= 64);
        if let Some(parent) = menu.parent_route_key() {
            assert!(preceding_routes.contains(parent));
        }
        preceding_routes.insert(menu.route_key.as_str());
    }
    let route_keys = menus
        .iter()
        .map(|menu| menu.route_key.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(route_keys.len(), menus.len());
    for menu in menus {
        if let Some(permission) = menu.permission.as_deref() {
            assert!(permission_set.contains(permission));
        }
    }
}

#[test]
fn generated_resource_access_is_owned_once_by_the_merged_seed_catalog() {
    let permissions = access_permission_codes().expect("合并权限目录应可解析");
    for permission in [
        "system:post:add",
        "system:post:edit",
        "system:post:export",
        "system:post:list",
        "system:post:remove",
    ] {
        assert_eq!(
            permissions
                .iter()
                .filter(|candidate| **candidate == permission)
                .count(),
            1,
            "生成岗位权限必须且只能归属一次: {permission}"
        );
    }

    let menus = access_menus().expect("合并菜单目录应可解析");
    let post_menus = menus
        .iter()
        .filter(|menu| menu.route_key == "system.post")
        .collect::<Vec<_>>();
    assert_eq!(post_menus.len(), 1, "生成岗位菜单必须且只能归属一次");
    assert_eq!(post_menus[0].name, "岗位管理");
    assert_eq!(
        post_menus[0].permission.as_deref(),
        Some("system:post:list")
    );
    assert_eq!(post_menus[0].parent_route_key(), Some("system"));

    let notice = menus
        .iter()
        .find(|menu| menu.route_key == "system.notice")
        .expect("生成通知菜单必须存在");
    assert_eq!(notice.name, "通知公告");
    assert_eq!(notice.permission.as_deref(), Some("system:notice:list"));
    assert_eq!(notice.parent_route_key(), Some("system"));
    assert_eq!(
        menus
            .iter()
            .find(|menu| menu.route_key == "system.perm")
            .expect("权限菜单必须存在")
            .sort(),
        16,
        "生成通知菜单不得改变后续手写菜单顺序"
    );
}

#[test]
fn review_snapshot_matches_the_fresh_schema() {
    let snapshot = mysql_snapshot_sql();
    assert!(snapshot.contains("schema fingerprint: c95b8a97fdbe6f49"));
    assert_eq!(snapshot.matches("CREATE TABLE IF NOT EXISTS").count(), 49);
    for required in [
        "`sys_background_job`",
        "`sys_background_job_attempt`",
        "`claim_sequence`",
        "`payload_version`",
        "`sys_export_job`",
        "`active_request_fingerprint`",
        "`delete_pending_at`",
        "`sys_backup_set`",
        "`sys_backup_resource`",
        "`sys_restore_run`",
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
    let actual_names = migrations
        .iter()
        .map(|migration| migration.name())
        .collect::<Vec<_>>();
    let expected_names = expected_migration_names().collect::<Vec<_>>();
    assert_eq!(actual_names, expected_names);
    assert_eq!(
        migrations.len(),
        1 + ryframe_db::generated::migrations().len()
    );
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
fn background_attempt_baseline_preserves_sequence_time_and_outcome_constraints() {
    let statements = control_ddl_statements().collect::<Vec<_>>();
    let background = statements
        .iter()
        .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_background_job`"))
        .expect("基线必须包含后台任务表");
    for required in [
        "`claim_sequence` BIGINT      NOT NULL DEFAULT 0",
        "`available_at`  DATETIME(6)",
        "`lease_until`   DATETIME(6)",
        "CONSTRAINT `ck_bg_job_attempt_budget`",
        "`claim_sequence` >= `attempts`",
    ] {
        assert!(
            background.contains(required),
            "后台任务缺少约束: {required}"
        );
    }

    let attempt = statements
        .iter()
        .find(|statement| {
            statement.contains("CREATE TABLE IF NOT EXISTS `sys_background_job_attempt`")
        })
        .expect("基线必须包含后台任务尝试表");
    for required in [
        "PRIMARY KEY (`job_id`, `sequence`)",
        "CONSTRAINT `fk_bg_attempt_job`",
        "CONSTRAINT `ck_bg_attempt_sequence`",
        "CONSTRAINT `ck_bg_attempt_times`",
        "CONSTRAINT `ck_bg_attempt_outcome`",
        "`outcome` = 'lease_expired' AND `finished_at` IS NULL",
    ] {
        assert!(attempt.contains(required), "尝试表缺少约束: {required}");
    }
}

#[test]
fn backup_baseline_preserves_state_and_relationship_contracts() {
    let statements = control_ddl_statements().collect::<Vec<_>>();
    let resource = statements
        .iter()
        .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_backup_resource`"))
        .expect("基线必须包含备份资源关系表");
    for required in [
        "PRIMARY KEY (`backup_id`, `resource_key`)",
        "KEY `idx_backup_resource_key` (`resource_key`, `backup_id`)",
        "CONSTRAINT `fk_backup_resource_set`",
    ] {
        assert!(
            resource.contains(required),
            "备份资源表缺少约束: {required}"
        );
    }

    let restore = statements
        .iter()
        .find(|statement| statement.contains("CREATE TABLE IF NOT EXISTS `sys_restore_run`"))
        .expect("基线必须包含恢复演练表");
    for required in [
        "KEY `idx_restore_run_backup` (`backup_id`, `started_at`)",
        "CONSTRAINT `fk_restore_run_backup`",
        "CHECK (`status` IN ('running', 'data_verified', 'succeeded', 'failed'))",
        concat!(
            "OR (`status` IN ('succeeded', 'failed')\n",
            "                    AND `completed_at` IS NOT NULL\n",
            "                    AND `completed_at` >= `started_at`))"
        ),
    ] {
        assert!(restore.contains(required), "恢复演练表缺少约束: {required}");
    }
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
    assert_eq!(count, 49);
    assert_eq!(schema_fingerprint(), "c95b8a97fdbe6f49");
}
