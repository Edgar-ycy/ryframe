use std::sync::atomic::{AtomicU64, Ordering};

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TryGetable};
use sea_orm_migration::MigratorTrait;

static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TEST_DATABASE_PREFIX: &str = "ryframe_migration_test_";
const V0_4_2_FIXTURE: &str = include_str!("fixtures/v0_4_2_mysql.sql");

#[tokio::test]
async fn empty_mysql_schema_is_initialized_and_idempotent() {
    let (admin, database, name) = isolated_database().await;

    ryframe_db_migration::run(&database).await.unwrap();
    ryframe_db_migration::run(&database).await.unwrap();

    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN ('sys_tenant', 'sys_user', 'sys_role', 'sys_permission', 'sys_menu', 'sys_file')",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(i64::try_get_by_index(&row, 0).unwrap(), 6);
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
    ryframe_db_migration::run(&database).await.unwrap();
    database
        .execute_unprepared("DROP TABLE `seaql_migrations`")
        .await
        .unwrap();

    ryframe_db_migration::run(&database).await.unwrap();

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

    ryframe_db_migration::run(&database).await.unwrap();
    ryframe_db_migration::run(&database).await.unwrap();
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
             AND reservation_token IS NULL AND reservation_expires_at IS NULL",
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(*) FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND COLUMN_NAME IN ('upload_status', 'reservation_token', 'reservation_expires_at')",
        )
        .await,
        3
    );
    assert_eq!(
        scalar_count(
            &database,
            "SELECT COUNT(DISTINCT INDEX_NAME) FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'sys_file' \
             AND INDEX_NAME IN ('idx_file_upload_reservation', 'idx_file_reservation_expiry')",
        )
        .await,
        2
    );

    cleanup_database(admin, database, &name).await;
}

#[tokio::test]
async fn complete_but_incompatible_schema_is_rejected() {
    let (admin, database, name) = isolated_database().await;
    ryframe_db_migration::run(&database).await.unwrap();
    database
        .execute_unprepared("DROP TABLE `seaql_migrations`")
        .await
        .unwrap();
    database
        .execute_unprepared("ALTER TABLE `sys_user` DROP COLUMN `password_hash`")
        .await
        .unwrap();

    let error = ryframe_db_migration::run(&database).await.unwrap_err();
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
    ryframe_db_migration::run(&database).await.unwrap();
    database
        .execute_unprepared(
            "DELETE FROM sys_config WHERE tenant_id = 'system' AND `key` = 'sys.index.skinName'",
        )
        .await
        .unwrap();

    ryframe_db_migration::run(&database).await.unwrap();
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
    ryframe_db_migration::run(&database).await.unwrap();
    database
        .execute_unprepared("UPDATE sys_config SET `key` = 'conflicting.key' WHERE id = 1")
        .await
        .unwrap();

    let error = ryframe_db_migration::run(&database).await.unwrap_err();
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
    ryframe_db_migration::run(&database).await.unwrap();
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
    ryframe_db_migration::run(&database).await.unwrap();
    database
        .execute_unprepared("ALTER TABLE sys_user MODIFY auth_version INT NOT NULL DEFAULT 2")
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
    assert!(error.contains("column sys_user.auth_version has default"));
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

    let error = ryframe_db_migration::run(&database).await.unwrap_err();
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

    let error = ryframe_db_migration::run(&database).await.unwrap_err();
    assert!(error.to_string().contains("partial RyFrame schema"));

    cleanup_database(admin, database, &name).await;
}

async fn isolated_database() -> (DatabaseConnection, DatabaseConnection, String) {
    let admin_url = std::env::var("RYFRAME_TEST_MYSQL_ADMIN_URL")
        .unwrap_or_else(|_| "mysql://root:ryframe_test_password@127.0.0.1:13306/mysql".into());
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
