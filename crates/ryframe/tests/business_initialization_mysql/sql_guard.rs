use super::*;

pub(super) async fn verify_non_check_value_boundaries(
    database: &DatabaseConnection,
) -> Result<(), String> {
    mysql::execute(database, r#"ALTER TABLE biz_sql_example MODIFY memo VARCHAR(32) NULL DEFAULT 'do-not-log-default|;:雪"tail', ADD parent_id BIGINT NULL, ADD CONSTRAINT fk_example_parent FOREIGN KEY(parent_id) REFERENCES biz_sql_example(id) ON DELETE SET NULL ON UPDATE CASCADE"#).await?;
    let actual = ryframe_db::schema::canonical_table_schema(database, "biz_sql_example")
        .await
        .map_err(mysql::db_error)?;
    let correct = ryframe_db::schema::schema_assertions("biz_sql_example", &actual)
        .map_err(mysql::db_error)?;
    assert_schema_predicates(database, &correct, &[]).await?;
    for (from, to, failed) in [
        ("do-not-log-default", "different-default", "columns"),
        (
            r#"Some("utf8mb4_general_ci")"#,
            r#"Some("utf8mb4_bin")"#,
            "columns",
        ),
        (r#"None:"YES""#, r#"None:"NO""#, "indexes"),
        (r#""CASCADE""#, r#""RESTRICT""#, "foreign_keys"),
    ] {
        assert!(actual.contains(from), "必须命中真实 metadata 的待变异字段");
        let changed = actual.replace(from, to);
        let plan = ryframe_db::schema::schema_assertions("biz_sql_example", &changed)
            .map_err(mysql::db_error)?;
        assert_schema_predicates(database, &plan, &[failed]).await?;
    }
    verify_non_check_guard_transaction(database, &correct, &actual).await
}

async fn verify_non_check_guard_transaction(
    database: &DatabaseConnection,
    correct: &ryframe_db::schema::SchemaAssertions,
    actual: &str,
) -> Result<(), String> {
    let transaction = database
        .begin_with_config(
            Some(sea_orm::IsolationLevel::Serializable),
            Some(sea_orm::AccessMode::ReadWrite),
        )
        .await
        .map_err(mysql::db_error)?;
    transaction
        .execute_unprepared(ryframe_db::schema::schema_assertion_guard())
        .await
        .map_err(mysql::db_error)?;
    let locks =
        ryframe_db::schema::schema_registration_locks(["biz_sql_example", "biz_sql_example"])
            .map_err(mysql::db_error)?;
    assert_eq!(locks.tables.len(), 1);
    for lock in locks.tables {
        transaction
            .query_all_raw(Statement::from_string(DbBackend::MySql, lock))
            .await
            .map_err(mysql::db_error)?;
    }
    for assertion in &correct.predicates {
        for sql in assertion.diagnostic_guard("fixture/source-export", "after") {
            transaction
                .execute_unprepared(&sql)
                .await
                .map_err(mysql::db_error)?;
        }
    }
    let changed = actual.replace("do-not-log-default", "different-default");
    let bad = ryframe_db::schema::schema_assertions("biz_sql_example", &changed)
        .map_err(mysql::db_error)?;
    let column = bad
        .predicates
        .iter()
        .find(|p| p.category == "columns")
        .ok_or("缺少列断言")?;
    let commands = column.diagnostic_guard("fixture/source-export", "after");
    assert_guard_location(&transaction, &commands[0]).await?;
    let error = transaction
        .execute_unprepared(&commands[1])
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ryframe_schema_assertion_failed"), "{error}");
    assert!(!error.contains("do-not-log-default") && !error.contains("different-default"));
    transaction
        .execute_unprepared("DROP TEMPORARY TABLE ryframe_schema_assertion_guard")
        .await
        .map_err(mysql::db_error)?;
    transaction.rollback().await.map_err(mysql::db_error)
}

async fn assert_guard_location<C: ConnectionTrait>(database: &C, sql: &str) -> Result<(), String> {
    let location = database
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await
        .map_err(mysql::db_error)?
        .ok_or("缺少 guard 定位结果")?;
    assert_eq!(
        location
            .try_get::<String>("", "migration")
            .map_err(mysql::db_error)?,
        "fixture/source-export"
    );
    assert_eq!(
        location
            .try_get::<String>("", "phase")
            .map_err(mysql::db_error)?,
        "after"
    );
    assert_eq!(
        location
            .try_get::<String>("", "schema_category")
            .map_err(mysql::db_error)?,
        "columns"
    );
    Ok(())
}

// 通过实际 metadata 执行生成的完整结构断言，不读取受测 SQL 文本。
pub(super) async fn assert_schema_predicates<C: ConnectionTrait>(
    database: &C,
    plan: &ryframe_db::schema::SchemaAssertions,
    expected_failures: &[&str],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for assertion in &plan.predicates {
        let row = database
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                format!("SELECT IF(({}), 1, 0) AS matches_schema", assertion.sql),
            ))
            .await
            .map_err(mysql::db_error)?
            .ok_or("结构断言缺少结果")?;
        let matches: i64 = row.try_get("", "matches_schema").map_err(mysql::db_error)?;
        if matches != 1 {
            failures.push(assertion.category);
        }
    }
    assert_eq!(failures, expected_failures);
    Ok(())
}

type SqlDisagreement = (
    &'static str,
    &'static [&'static str],
    Option<&'static [&'static str]>,
    i64,
    &'static str,
);
const SQL_DISAGREEMENTS: [SqlDisagreement; 5] = [
    (
        "sqlbadcreate",
        &[
            "CREATE TABLE biz_sql_example (id BIGINT NOT NULL AUTO_INCREMENT, code VARCHAR(64) NOT NULL, PRIMARY KEY(id), UNIQUE KEY uk_example_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci",
        ],
        None,
        0,
        "SELECT COUNT(*) AS value FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='biz_sql_example' AND column_name='code' AND column_type='varchar(64)'",
    ),
    (
        "sqlbadalter",
        &[DDL],
        Some(&["ALTER TABLE biz_sql_example ADD memo VARCHAR(64) NULL"]),
        1,
        "SELECT COUNT(*) AS value FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='biz_sql_example' AND column_name='memo' AND column_type='varchar(64)'",
    ),
    (
        "sqlbadledger",
        &[DDL],
        Some(&[
            "ALTER TABLE biz_sql_example ADD memo VARCHAR(32) NULL",
            "UPDATE ryframe_business_migration SET checksum=REPEAT('b',64)",
        ]),
        1,
        "SELECT COUNT(*) AS value FROM ryframe_business_migration WHERE checksum=REPEAT('b',64)",
    ),
    (
        "sqlbadframework",
        &[
            DDL,
            "ALTER TABLE sys_user MODIFY username VARCHAR(65) NOT NULL",
        ],
        None,
        0,
        "SELECT COUNT(*) AS value FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='sys_user' AND column_name='username' AND column_type='varchar(65)'",
    ),
    (
        "sqlbadcheck",
        &[
            DDL,
            "ALTER TABLE sys_restore_run ALTER CHECK ck_restore_run_completed NOT ENFORCED",
        ],
        None,
        0,
        "SELECT COUNT(*) AS value FROM information_schema.table_constraints WHERE constraint_schema=DATABASE() AND table_name='sys_restore_run' AND constraint_name='ck_restore_run_completed' AND enforced='NO'",
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_create_alter_and_ledger_disagreements_never_register_completion() {
    for (label, initial_sql, next_sql, recorded, reached) in SQL_DISAGREEMENTS {
        mysql::run_mysql_test(label, move |database| async move {
            let initial = migration(false).with_initialization_sql(initial_sql);
            let next = next_sql.map(|sql| next_migration().with_initialization_sql(sql));
            let mut migrations: Vec<&dyn BusinessMigration> = vec![&initial];
            if let Some(next) = &next {
                migrations.push(next);
            }
            let sql = ryframe::business::shared_control_initialization_sql(&hashes()?, &migrations)
                .map_err(|e| e.to_string())?;
            let error = database
                .execute_unprepared(&sql)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("ryframe_schema_assertion_failed"), "{error}");
            // 必须已执行对应的合法 SQL 才能说明错误账本被结构/归属断言阻止。
            mysql::require_count(&database, reached, 1).await?;
            mysql::require_count(
                &database,
                "SELECT COUNT(*) AS value FROM ryframe_business_migration",
                recorded,
            )
            .await?;
            mysql::require_count(
                &database,
                "SELECT COUNT(*) AS value FROM seaql_migrations",
                0,
            )
            .await?;
            mysql::require_count(
                &database,
                "SELECT COUNT(*) AS value FROM seaql_tenant_data_migrations",
                0,
            )
            .await?;
            Ok(())
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_valid_incremental_chain_records_only_verified_shapes() {
    mysql::run_mysql_test("sqlguardchain", |database| async move {
        let initial = migration(false).with_initialization_sql(&[DDL]);
        let next = next_migration()
            .with_initialization_sql(&["ALTER TABLE biz_sql_example ADD memo VARCHAR(32) NULL"]);
        let sql =
            ryframe::business::shared_control_initialization_sql(&hashes()?, &[&initial, &next])
                .map_err(|e| e.to_string())?;
        // 完整导出含会话设置、临时表和登记事务，必须在同一连接以多语句执行。
        database
            .execute_unprepared(&sql)
            .await
            .map_err(mysql::db_error)?;
        verify_business_migrations(
            &database,
            BusinessMigrationScope::Control,
            &[&initial, &next],
        )
        .await
        .map_err(|e| e.to_string())?;
        assert_eq!(
            ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example")
                .await
                .map_err(mysql::db_error)?,
            AFTER_SCHEMA
        );
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM ryframe_business_migration",
            2,
        )
        .await?;
        assert!(
            ryframe_db::migration::status(&database)
                .await
                .map_err(mysql::db_error)?
                .is_up_to_date()
        );
        assert!(
            ryframe_tenant_db::migration::status(&database)
                .await
                .map_err(mysql::db_error)?
                .is_up_to_date()
        );
        Ok(())
    })
    .await;
}
