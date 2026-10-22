use super::{bootstrap_hashes, db_error, execute, require_count, run_mysql_test};
use ryframe_db::migration;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_import_and_migrations_produce_equivalent_control_databases() {
    run_mysql_test("sql-init", |imported| async move {
        let sql = migration::mysql_initialization_sql(&bootstrap_hashes());
        imported.execute_unprepared(&sql).await.map_err(db_error)?;
        migration::verify(&imported).await.map_err(db_error)?;
        require_count(
            &imported,
            "SELECT COUNT(*) AS value FROM sys_user WHERE password_change_required = 1",
            2,
        )
        .await?;
        // 已初始化库不再需要初始密码，也不能把已改密账号重新置为必须改密。
        execute(
            &imported,
            "UPDATE sys_user SET password_change_required = 0 WHERE id = 2",
        )
        .await?;
        migration::up(&imported, None).await.map_err(db_error)?;
        require_count(
            &imported,
            "SELECT COUNT(*) AS value FROM sys_user WHERE id = 2 AND password_change_required = 0",
            1,
        )
        .await?;
        execute(
            &imported,
            "UPDATE sys_user SET password_change_required = 1 WHERE id = 2",
        )
        .await?;
        run_mysql_test("migrate-init", move |migrated| async move {
            migration::up(&migrated, Some(&bootstrap_hashes()))
                .await
                .map_err(db_error)?;
            migration::verify(&migrated).await.map_err(db_error)?;
            assert_eq!(inventory(&imported).await?, inventory(&migrated).await?);
            Ok(())
        })
        .await;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_bootstrap_password_does_not_create_tables() {
    run_mysql_test("init-no-secret", |database| async move {
        let error = migration::up(&database, None).await.expect_err("首次初始化必须显式提供密码");
        assert!(error.to_string().contains("[bootstrap]") && error.to_string().contains("admin_password"));
        require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema = DATABASE()", 0).await
    }).await;
}

async fn inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'column', COLUMN_NAME, 'type', COLUMN_TYPE, 'nullable', IS_NULLABLE, 'default', COLUMN_DEFAULT, 'extra', EXTRA, 'collation', COLLATION_NAME) AS CHAR) AS item FROM information_schema.columns WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION",
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'index', INDEX_NAME, 'unique', NON_UNIQUE, 'sequence', SEQ_IN_INDEX, 'column', COLUMN_NAME) AS CHAR) AS item FROM information_schema.statistics WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        "SELECT CAST(JSON_OBJECT('table', TABLE_NAME, 'column', COLUMN_NAME, 'parent', REFERENCED_TABLE_NAME, 'parent_column', REFERENCED_COLUMN_NAME) AS CHAR) AS item FROM information_schema.key_column_usage WHERE TABLE_SCHEMA = DATABASE() AND REFERENCED_TABLE_NAME IS NOT NULL ORDER BY TABLE_NAME, COLUMN_NAME",
        "SELECT CAST(JSON_OBJECT('tenant', tenant_id, 'username', username, 'status', status, 'change_password', password_change_required) AS CHAR) AS item FROM sys_user ORDER BY tenant_id, username",
        "SELECT CAST(JSON_OBJECT('tenant', tenant_id, 'code', code, 'name', name, 'status', status) AS CHAR) AS item FROM sys_permission ORDER BY tenant_id, code",
        "SELECT CAST(JSON_OBJECT('route', m.route_key, 'name', m.name, 'parent', p.route_key, 'permission', a.code, 'icon', m.icon, 'sort', m.sort) AS CHAR) AS item FROM sys_menu m LEFT JOIN sys_menu p ON m.parent_id = p.id LEFT JOIN sys_permission a ON m.perm_id = a.id ORDER BY m.tenant_id, m.route_key",
    ] {
        let rows = database
            .query_all_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
            .await
            .map_err(db_error)?;
        result.push(
            rows.into_iter()
                .map(|row| row.try_get("", "item").map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(result)
}
