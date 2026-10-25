use ryframe_db::migration;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

use super::mysql::{execute, run_mysql_test};

const RESTORE_CHECK: &str = "ck_restore_run_completed";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_schema_checks_are_compared_by_expression_and_enforcement() {
    run_mysql_test("control-checks", |database| async move {
        migration::up(&database, Some(&super::bootstrap_hashes()))
            .await
            .map_err(|error| format!("安装规范控制库失败: {error}"))?;
        migration::verify_current_schema(&database)
            .await
            .map_err(|error| format!("规范控制库 CHECK 校验失败: {error}"))?;

        compare_check_normalizers(&database).await?;
        assert_sql_categories(&database, "sys_restore_run", &[]).await?;
        replace_restore_check(&database, false).await?;
        assert_sql_categories(&database, "sys_restore_run", &["checks"]).await?;
        require_verification_error(&database, "does not match canonical expression").await?;

        replace_restore_check(&database, true).await?;
        assert_sql_categories(&database, "sys_restore_run", &["constraints"]).await?;
        require_verification_error(&database, "enforcement is NOT ENFORCED, expected ENFORCED")
            .await?;

        execute(
            &database,
            "ALTER TABLE `sys_background_job` DROP CHECK `ck_bg_job_attempt_budget`",
        )
        .await?;
        assert_sql_categories(&database, "sys_background_job", &["checks", "constraints"]).await?;
        require_verification_error(
            &database,
            "missing check constraint sys_background_job.ck_bg_job_attempt_budget",
        )
        .await
    })
    .await;
}

async fn replace_restore_check(
    database: &DatabaseConnection,
    canonical_but_disabled: bool,
) -> Result<(), String> {
    execute(
        database,
        &format!("ALTER TABLE `sys_restore_run` DROP CHECK `{RESTORE_CHECK}`"),
    )
    .await?;
    let expression = if canonical_but_disabled {
        "((`status` IN ('running', 'data_verified') AND `completed_at` IS NULL) \
         OR (`status` IN ('succeeded', 'failed') AND `completed_at` IS NOT NULL \
         AND `completed_at` >= `started_at`)) NOT ENFORCED"
    } else {
        "((`status` IN ('running', 'data_verified') AND `completed_at` IS NULL) \
         OR (`status` IN ('succeeded', 'failed') AND `completed_at` >= `started_at`))"
    };
    execute(
        database,
        &format!(
            "ALTER TABLE `sys_restore_run` ADD CONSTRAINT `{RESTORE_CHECK}` CHECK {expression}"
        ),
    )
    .await
}

async fn require_verification_error(
    database: &DatabaseConnection,
    expected: &str,
) -> Result<(), String> {
    let error = migration::verify_current_schema(database)
        .await
        .expect_err("被篡改的 CHECK 必须校验失败")
        .to_string();
    if error.contains(expected) {
        Ok(())
    } else {
        Err(format!("CHECK 校验错误缺少 {expected:?}: {error}"))
    }
}

async fn assert_sql_categories(
    database: &DatabaseConnection,
    table: &str,
    expected: &[&str],
) -> Result<(), String> {
    let plans = migration::framework_initialization_assertions(&[]).map_err(|e| e.to_string())?;
    let (_, plan) = plans
        .into_iter()
        .find(|(name, _)| name == table)
        .ok_or("缺少正式框架结构断言")?;
    let transaction = database.begin().await.map_err(|e| e.to_string())?;
    let previous = recursion_depth(&transaction).await?;
    transaction
        .execute_unprepared("SET SESSION cte_max_recursion_depth=4294967295")
        .await
        .map_err(|e| e.to_string())?;
    let mut failed = Vec::new();
    for p in plan.predicates {
        let row = transaction
            .query_one_raw(Statement::from_string(
                DbBackend::MySql,
                format!("SELECT IF(({}),1,0) AS passed", p.sql),
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or("结构断言没有结果")?;
        if row
            .try_get::<i64>("", "passed")
            .map_err(|e| e.to_string())?
            != 1
        {
            failed.push(p.category);
        }
    }
    transaction
        .execute_unprepared(&format!("SET SESSION cte_max_recursion_depth={previous}"))
        .await
        .map_err(|e| e.to_string())?;
    transaction.rollback().await.map_err(|e| e.to_string())?;
    assert_eq!(failed, expected);
    Ok(())
}

async fn compare_check_normalizers(database: &DatabaseConnection) -> Result<(), String> {
    let transaction = database.begin().await.map_err(|e| e.to_string())?;
    let previous = recursion_depth(&transaction).await?;
    transaction
        .execute_unprepared("SET SESSION cte_max_recursion_depth=4294967295")
        .await
        .map_err(|e| e.to_string())?;
    let mut cases = vec![
        r#"(`state` IN (_utf8mb4\'active\', _ascii\'failed\'))"#.to_owned(),
        "((state in ('active', 'failed')))".to_owned(),
        r#"(`code` = _utf8mb4\'O\\\'Reilly\')"#.to_owned(),
        "`code` = 'O''Reilly'".to_owned(),
        "`code` = 'A B'".to_owned(), "`code` = 'a b'".to_owned(),
        "((`a` AND `b`) OR `c`)".to_owned(), "(`a` AND (`b` OR `c`))".to_owned(),
        "`a-b` > 0".to_owned(), "`a` - `b` > 0".to_owned(), "`a` IS NULL".to_owned(), "`aisnull`".to_owned(),
        "TRUE".to_owned(), "`true`".to_owned(),
        "((`attempts` >= 0) and (`max_attempts` between 1 and 100) and (`claim_sequence` >= `attempts`))".to_owned(),
        r#"((`status` in (_utf8mb4\'running\',_utf8mb4\'failed\')) and (`completed_at` is null))"#.to_owned(),
        "(a)(b)".to_owned(), "(a between 1 and 2) and f((b+1),c)".to_owned(),
        "`列Ä` >= 0xAF + 0b10 + .5E-2 AND `a``b` <> '雪|;:'".to_owned(),
    ];
    cases.push(format!("code='{}'", "long_literal".repeat(200)));
    let expression = migration::normalize_check_clause_sql("?");
    for input in cases {
        let row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!("SELECT {expression} AS normalized"),
                [input.clone().into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .ok_or("词法断言没有结果")?;
        let actual: String = row.try_get("", "normalized").map_err(|e| e.to_string())?;
        assert_eq!(
            actual,
            migration::normalize_check_clause(&input),
            "CHECK 两路径不一致"
        );
    }
    transaction
        .execute_unprepared(&format!("SET SESSION cte_max_recursion_depth={previous}"))
        .await
        .map_err(|e| e.to_string())?;
    transaction.rollback().await.map_err(|e| e.to_string())
}

async fn recursion_depth<C: ConnectionTrait>(database: &C) -> Result<u64, String> {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CAST(@@SESSION.cte_max_recursion_depth AS UNSIGNED) AS depth",
        ))
        .await
        .map_err(|e| e.to_string())?
        .ok_or("缺少递归深度")?
        .try_get("", "depth")
        .map_err(|e| e.to_string())
}
