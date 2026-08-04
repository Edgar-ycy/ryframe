use std::sync::atomic::{AtomicU64, Ordering};

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TryGetable};
use sea_orm_migration::{MigratorTrait, SchemaManager};

static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TEST_DATABASE_PREFIX: &str = "ryframe_migration_test_";
const V0_4_2_FIXTURE: &str = include_str!("fixtures/v0_4_2_mysql.sql");

#[tokio::test]
async fn empty_mysql_schema_is_initialized_and_idempotent() {
    let (admin, database, name) = isolated_database().await;

    ryframe_db_migration::up(&database).await.unwrap();
    ryframe_db_migration::up(&database).await.unwrap();

    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('sys_tenant', 'sys_user', 'sys_role', 'sys_permission', 'sys_menu', 'sys_file')",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 6);
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = 'sys_outbox_event'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(DISTINCT INDEX_NAME) FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = 'sys_outbox_event' \
             AND INDEX_NAME IN ('uq_outbox_event_dedupe', 'idx_outbox_event_claim', \
                                'idx_outbox_event_lease', 'idx_outbox_event_aggregate')",
        )
        .await,
        4
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = 'sys_export_job'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(DISTINCT INDEX_NAME) FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = 'sys_export_job' \
             AND INDEX_NAME IN ('uq_export_job_background', 'idx_export_job_requester', \
                                'idx_export_job_expiry')",
        )
        .await,
        3
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.columns \
             WHERE table_schema = DATABASE() \
             AND ((table_name = 'sys_message' AND column_name IN \
                   ('published_at', 'expires_at', 'created_at', 'updated_at')) \
               OR (table_name = 'sys_message_recipient' AND column_name IN \
                   ('created_at', 'enqueued_at', 'acked_at', 'read_at'))) \
             AND column_type = 'datetime(6)'",
        )
        .await,
        8
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_cache_namespace_version \
             WHERE tenant_id = 'system' AND namespace = 'config' AND version = 0",
        )
        .await,
        1
    );
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM sys_user WHERE tenant_id = 'system' AND username IN ('admin', 'user')",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 2);

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn complete_schema_without_migration_ledger_is_verified_and_registered() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared("DROP TABLE `seaql_migrations`")
        .await
        .unwrap();

    ryframe_db_migration::up(&database).await.unwrap();

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn tagged_v0_4_schema_and_data_upgrade_is_lossless_idempotent_and_canonical() {
    let (admin, database, name) = isolated_database().await;
    execute_sql_fixture(&database, V0_4_2_FIXTURE).await;
    database
        .execute_unprepared(
            "INSERT INTO sys_file \
             (id, tenant_id, original_name, storage_name, storage_path, bucket, file_url, \
              file_size, content_type, file_md5, upload_by, del_flag) \
             VALUES \
             (9002, 'legacy-fixture', 'legacy.txt', 'legacy.txt', \
              'legacy-fixture/legacy.txt', 'uploads', 'uploads/legacy-fixture/legacy.txt', \
              6, 'text/plain', '228c70bfc5589c58c044e03fff0e17eb', 'legacy', '0')",
        )
        .await
        .unwrap();

    let error = ryframe_db_migration::up(&database).await.unwrap_err();
    assert!(
        error.to_string().contains("backfill-sha256"),
        "迁移必须在摘要维护未完成时闭锁失败，实际错误: {error}"
    );
    database
        .execute_unprepared(
            "UPDATE sys_file \
             SET file_sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
             WHERE id = 9002",
        )
        .await
        .unwrap();

    ryframe_db_migration::up(&database).await.unwrap();
    ryframe_db_migration::up(&database).await.unwrap();
    ryframe_db_migration::verify_current_schema(&database)
        .await
        .unwrap();

    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_tenant WHERE id = 9000 AND tenant_id = 'legacy-fixture' AND name = 'Legacy fixture tenant'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_config WHERE id = 9001 AND tenant_id = 'legacy-fixture' AND `key` = 'legacy.custom' AND value = 'keep-me'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_cache_namespace_version \
             WHERE tenant_id = 'legacy-fixture' AND namespace = 'config' AND version = 0",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(&database, "SELECT COUNT(*) FROM seaql_migrations").await,
        ryframe_db_migration::Migrator::migrations().len() as i64
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.TABLE_CONSTRAINTS \
             WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_user_role' \
             AND CONSTRAINT_TYPE = 'FOREIGN KEY'",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_file \
             WHERE id = 9002 AND upload_status = 'ready' \
             AND reservation_token IS NULL AND reservation_expires_at IS NULL \
             AND file_sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND COLUMN_NAME IN ('upload_status', 'reservation_token', 'reservation_expires_at', 'file_sha256')",
        )
        .await,
        4
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(DISTINCT INDEX_NAME) FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND INDEX_NAME IN ('idx_file_reservation_expiry', 'idx_file_sha256')",
        )
        .await,
        2
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND COLUMN_NAME = 'file_sha256' AND IS_NULLABLE = 'NO'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND COLUMN_NAME = 'file_md5'",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND INDEX_NAME = 'idx_file_upload_reservation'",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_user' \
               AND COLUMN_NAME = 'authorization_version'",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_user' \
               AND COLUMN_NAME = 'auth_version'",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_user \
             WHERE tenant_id = 'legacy-fixture' AND username = 'legacy-user' \
               AND authorization_version = 7",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_tenant \
             WHERE tenant_id = 'legacy-fixture' AND authorization_epoch = 1",
        )
        .await,
        1
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn platform_message_permission_repair_removes_only_non_system_grants() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared(
            "INSERT INTO sys_tenant (id, tenant_id, name, status) \
             VALUES (700000001, 'tenant-platform-leak', '历史权限租户', '1')",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "INSERT INTO sys_role \
             (id, tenant_id, name, code, is_super, data_scope, status, sort) \
             VALUES (700000002, 'tenant-platform-leak', '历史管理员', 'legacy-admin', 1, '1', '1', 1)",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "INSERT INTO sys_permission \
             (id, tenant_id, name, code, parent_id, perm_type, sort, status) \
             VALUES (700000003, 'tenant-platform-leak', '跨租户发布消息', \
                     'platform:message:publish', NULL, 'api', 1, '1')",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "INSERT INTO sys_role_permission (tenant_id, role_id, perm_id) \
             VALUES ('tenant-platform-leak', 700000002, 700000003)",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "INSERT INTO sys_menu \
             (id, tenant_id, name, menu_type, perm_id, sort, visible, status, del_flag) \
             VALUES (700000004, 'tenant-platform-leak', '历史跨租户发布', 'F', 700000003, 1, 1, '1', '0')",
        )
        .await
        .unwrap();

    let migration = ryframe_db_migration::Migrator::migrations()
        .into_iter()
        .find(|migration| migration.name() == "m20260726_000011_platform_message_permission_scope")
        .expect("platform permission repair migration");
    migration
        .up(&SchemaManager::new(&database))
        .await
        .expect("repair historical platform permission");

    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_permission \
             WHERE tenant_id = 'tenant-platform-leak' \
               AND code = 'platform:message:publish'",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_role_permission \
             WHERE tenant_id = 'tenant-platform-leak' AND perm_id = 700000003",
        )
        .await,
        0
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_menu \
             WHERE id = 700000004 AND perm_id IS NULL",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM sys_permission \
             WHERE tenant_id = 'system' AND code = 'platform:message:publish'",
        )
        .await,
        1
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn message_time_precision_migration_upgrades_existing_tables() {
    let (admin, database, name) = isolated_database().await;
    database
        .execute_unprepared(
            "CREATE TABLE sys_message (\
                 id BIGINT NOT NULL PRIMARY KEY, \
                 published_at DATETIME NOT NULL, \
                 expires_at DATETIME DEFAULT NULL, \
                 created_at DATETIME NOT NULL, \
                 updated_at DATETIME NOT NULL\
             ) ENGINE=InnoDB",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "CREATE TABLE sys_message_recipient (\
                 message_id BIGINT NOT NULL, \
                 user_id BIGINT NOT NULL, \
                 created_at DATETIME NOT NULL, \
                 enqueued_at DATETIME DEFAULT NULL, \
                 acked_at DATETIME DEFAULT NULL, \
                 read_at DATETIME DEFAULT NULL, \
                 PRIMARY KEY (message_id, user_id)\
             ) ENGINE=InnoDB",
        )
        .await
        .unwrap();

    let migration = ryframe_db_migration::Migrator::migrations()
        .into_iter()
        .find(|migration| migration.name() == "m20260805_000020_message_time_precision")
        .expect("message time precision migration");
    migration
        .up(&SchemaManager::new(&database))
        .await
        .expect("upgrade existing message timestamp columns");

    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.columns \
             WHERE table_schema = DATABASE() \
             AND ((table_name = 'sys_message' AND column_name IN \
                   ('published_at', 'expires_at', 'created_at', 'updated_at')) \
               OR (table_name = 'sys_message_recipient' AND column_name IN \
                   ('created_at', 'enqueued_at', 'acked_at', 'read_at'))) \
             AND column_type = 'datetime(6)'",
        )
        .await,
        8
    );
    database
        .execute_unprepared(
            "INSERT INTO sys_message \
             (id, published_at, expires_at, created_at, updated_at) VALUES \
             (1, '2026-08-05 12:34:56.654321', NULL, \
              '2026-08-05 12:34:56.654321', '2026-08-05 12:34:56.654321')",
        )
        .await
        .unwrap();
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATE_FORMAT(published_at, '%Y-%m-%d %H:%i:%s.%f') \
             FROM sys_message WHERE id = 1"
                .to_owned(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        String::try_get_by_index(&row, 0).unwrap(),
        "2026-08-05 12:34:56.654321"
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn complete_but_incompatible_schema_is_rejected() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared("DROP TABLE `seaql_migrations`")
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE `sys_user` DROP COLUMN `password_hash`")
        .await
        .unwrap();

    let error = ryframe_db_migration::up(&database).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing column sys_user.password_hash")
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn missing_seed_row_is_restored_idempotently() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared(
            "DELETE FROM sys_config WHERE tenant_id = 'system' AND `key` = 'sys.index.skinName'",
        )
        .await
        .unwrap();

    ryframe_db_migration::up(&database).await.unwrap();
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM sys_config WHERE tenant_id = 'system' AND `key` = 'sys.index.skinName'".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 1);

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn conflicting_seed_identity_is_rejected_instead_of_silently_ignored() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared("UPDATE sys_config SET `key` = 'conflicting.key' WHERE id = 1")
        .await
        .unwrap();

    let error = ryframe_db_migration::up(&database).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("canonical seed identity is missing or conflicting in sys_config")
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn canonical_fingerprint_rejects_extra_application_objects() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared("CREATE TABLE unexpected_app_table (id BIGINT PRIMARY KEY)")
        .await
        .unwrap();
    database
        .execute_unprepared(
            "ALTER TABLE sys_config ADD COLUMN unexpected_column VARCHAR(8) NULL, \
             ADD INDEX unexpected_index (unexpected_column)",
        )
        .await
        .unwrap();
    database
        .execute_unprepared(
            "ALTER TABLE sys_user ADD CONSTRAINT unexpected_user_dept_fk \
             FOREIGN KEY (dept_id) REFERENCES sys_dept(id) ON DELETE SET NULL ON UPDATE CASCADE",
        )
        .await
        .unwrap();

    let error = ryframe_db_migration::verify_current_schema(&database)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("unexpected application table unexpected_app_table"));
    assert!(error.contains("unexpected column sys_config.unexpected_column"));
    assert!(error.contains("unexpected index sys_config.unexpected_index"));
    assert!(error.contains("unexpected foreign key sys_user.unexpected_user_dept_fk"));

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn canonical_fingerprint_rejects_engine_column_table_collation_and_fk_action_drift() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::up(&database).await.unwrap();
    database
        .execute_unprepared(
            "ALTER TABLE sys_user MODIFY authorization_version INT NOT NULL DEFAULT 2",
        )
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE sys_config COLLATE utf8mb4_bin")
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE sys_config DROP FOREIGN KEY fk_sys_config_tenant")
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE sys_config ENGINE=MyISAM")
        .await
        .unwrap();
    database
        .execute_unprepared(
            "ALTER TABLE sys_user MODIFY username VARCHAR(64) CHARACTER SET utf8mb4 \
             COLLATE utf8mb4_bin NOT NULL COMMENT '用户名'",
        )
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE sys_role_dept DROP FOREIGN KEY fk_sys_role_dept_role")
        .await
        .unwrap();
    database
        .execute_unprepared(
            "ALTER TABLE sys_role_dept ADD CONSTRAINT fk_sys_role_dept_role \
             FOREIGN KEY (role_id) REFERENCES sys_role(id) \
             ON DELETE RESTRICT ON UPDATE CASCADE",
        )
        .await
        .unwrap();

    let error = ryframe_db_migration::verify_current_schema(&database)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("column sys_user.authorization_version has default"));
    assert!(error.contains("table sys_config uses engine myisam"));
    assert!(error.contains("table sys_config has collation utf8mb4_bin"));
    assert!(error.contains("column sys_user.username has collation"));
    assert!(error.contains("foreign key sys_role_dept.fk_sys_role_dept_role"));

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn non_empty_unrelated_database_is_rejected() {
    let (admin, database, name) = isolated_database().await;
    database
        .execute_unprepared("CREATE TABLE unrelated_business_data (id BIGINT PRIMARY KEY)")
        .await
        .unwrap();

    let error = ryframe_db_migration::up(&database).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("database is not empty and does not contain a RyFrame schema")
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn partial_schema_is_rejected() {
    let (admin, database, name) = isolated_database().await;
    database
        .execute_unprepared("CREATE TABLE sys_user (id BIGINT NOT NULL PRIMARY KEY)")
        .await
        .unwrap();

    let error = ryframe_db_migration::up(&database).await.unwrap_err();
    assert!(error.to_string().contains("partial RyFrame schema"));

    cleanup_database(admin, database, &name).await;
}

async fn isolated_database() -> (DatabaseConnection, DatabaseConnection, String) {
    ryframe_utils::snowflake::initialize(1).expect("初始化测试 Snowflake");
    let admin_url = mysql_test_admin_url();
    let admin = Database::connect(&admin_url).await.expect(
        "connect MySQL test service; run `docker compose -f docker-compose.test.yml up -d --wait`",
    );
    let name = format!(
        "{TEST_DATABASE_PREFIX}{}_{}",
        std::process::id(),
        DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    admin
        .execute_unprepared(&format!(
            "CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci"
        ))
        .await
        .unwrap();
    let prefix = admin_url.rsplit_once('/').unwrap().0;
    let database = Database::connect(format!("{prefix}/{name}?collation=utf8mb4_general_ci"))
        .await
        .unwrap();
    (admin, database, name)
}

fn mysql_test_admin_url() -> String {
    if let Ok(admin_url) = std::env::var("RYFRAME_TEST_MYSQL_ADMIN_URL") {
        return admin_url;
    }
    let port = std::env::var("RYFRAME_TEST_MYSQL_PORT")
        .ok()
        .map_or(13306, |value| {
            value
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .unwrap_or_else(|| panic!("RYFRAME_TEST_MYSQL_PORT 必须是 1 到 65535 之间的端口号"))
        });
    format!("mysql://root:ryframe_test_password@127.0.0.1:{port}/mysql")
}

async fn cleanup_database(admin: DatabaseConnection, database: DatabaseConnection, name: &str) {
    assert!(name.starts_with(TEST_DATABASE_PREFIX));
    assert!(
        name.chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    );
    database.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP DATABASE `{name}`"))
        .await
        .unwrap();
}

async fn execute_sql_fixture(database: &DatabaseConnection, fixture: &str) {
    let mut statement = String::new();
    for line in fixture.lines() {
        let line = strip_sql_line_comment(line);
        if line.trim().is_empty() {
            continue;
        }
        statement.push_str(line);
        statement.push('\n');
        if line.trim_end().ends_with(';') {
            let sql = statement.trim().trim_end_matches(';').trim();
            if !sql.is_empty() {
                database.execute_unprepared(sql).await.unwrap();
            }
            statement.clear();
        }
    }
    assert!(
        statement.trim().is_empty(),
        "fixture has an unterminated SQL statement"
    );
}

fn strip_sql_line_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut characters = line.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
            } else {
                quoted = !quoted;
            }
        } else if character == '-'
            && !quoted
            && characters.peek().is_some_and(|(_, next)| *next == '-')
        {
            return &line[..index];
        }
    }
    line
}

async fn scalar_count(database: &DatabaseConnection, sql: &str) -> i64 {
    let row = database
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await
        .unwrap()
        .unwrap();
    i64::try_get_by_index(&row, 0).unwrap()
}
