use super::*;
use ryframe_sdk::{
    BusinessAdoptionPlan, BusinessAdoptionState, BusinessMigrationOrigin, apply_business_adoption,
    inspect_business_adoption, preview_business_adoption,
};

#[test]
fn preview_and_adopt_preserve_existing_schema_and_rows() {
    // 业务访问目录只能登记一次；真实子进程隔离它，避免污染同 bin 的并行 SQL 用例。
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adoption::preview_and_adopt_with_unseeded_access_in_isolated_process",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "隔离接管回归未完整执行通过：{}",
        output.status
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由现有父用例实际运行，隔离进程级业务访问目录"]
async fn preview_and_adopt_with_unseeded_access_in_isolated_process() {
    ryframe_db::migration::register_business_tables([("biz_sql_example", TABLE_SCHEMA)]).unwrap();
    mysql::run_mysql_test("adoptbaseline", |database| async move {
        ryframe_db::migration::up(&database, Some(&hashes()?)).await.map_err(mysql::db_error)?;
        let catalog = ryframe_db::migration::register_business_access_catalog([
            ("order", include_str!("../../../ryframe-sdk/tests/fixtures/order_access.toml"))
        ]).map_err(mysql::db_error)?;
        let access_before = access_rows(&database).await?;
        mysql::execute(&database, DDL).await?;
        mysql::execute(&database, "INSERT INTO biz_sql_example(code) VALUES ('private-business-row')").await?;
        let rows = business_rows(&database).await?;
        let initial = migration(false);
        let failure = ryframe_db::migration::verify(&database).await.unwrap_err().to_string();
        assert!(failure.contains("业务权限") && catalog.permission_names().keys().any(|code| failure.contains(code.as_str())), "{failure}");
        assert!(initial.up(&database).await.is_err());
        ryframe_db::migration::verify_control_baseline(&database).await.map_err(mysql::db_error)?;
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.map_err(message)?;
        assert!(!plan.manifest.ledger.existed);
        let encoded = serde_json::to_string(&plan).map_err(message)?;
        assert!(!encoded.contains("private-business-row") && !encoded.contains("CREATE TABLE") && !encoded.contains("columns="));
        let plan: BusinessAdoptionPlan = serde_json::from_str(&encoded).map_err(message)?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'", 0).await?;
        assert_eq!(inspect_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?, BusinessAdoptionState::Unapplied);
        let outcome = apply_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?;
        assert_eq!(outcome.registered, 1);
        assert!(outcome.metadata_ddl_attempted && !outcome.already_applied);
        assert_eq!(initial.status(&database).await.map_err(message)?.origin, Some(BusinessMigrationOrigin::Adopted));
        verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial]).await.map_err(message)?;
        assert_eq!(business_rows(&database).await?, rows);
        ryframe_db::migration::verify_control_baseline(&database).await.map_err(mysql::db_error)?;
        let before = inventory(&database).await?;
        let time = timestamps(&database).await?;
        assert_eq!(inspect_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?, BusinessAdoptionState::Applied);
        let repeated = apply_business_adoption(&database, &plan, &[&initial]).await.map_err(message)?;
        assert!(repeated.already_applied && !repeated.metadata_ddl_attempted);
        assert_eq!(repeated.registered, 0);
        initial.up(&database).await.map_err(message)?;
        assert_eq!(inventory(&database).await?, before);
        assert_eq!(timestamps(&database).await?, time);
        assert!(preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.unwrap_err().to_string().contains("不能重新接管"));
        assert_eq!(access_rows(&database).await?, access_before);
        assert!(ryframe_db::migration::verify(&database).await.unwrap_err().to_string().contains("业务权限"));
        ryframe_db::migration::up(&database, None).await.map_err(mysql::db_error)?;
        initial.up(&database).await.map_err(message)?;
        ryframe_db::migration::verify(&database).await.map_err(mysql::db_error)?;
        assert_eq!(business_rows(&database).await?, rows);
        assert_eq!(inventory(&database).await?, before);
        assert_eq!(timestamps(&database).await?, time);
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_rejects_tampering_source_changes_and_schema_drift_before_writes() {
    mysql::run_mysql_test("adoptconflict", |database| async move {
        mysql::execute(&database, DDL).await?;
        let initial = migration(false);
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.map_err(message)?;
        let mut changed = plan.clone();
        changed.manifest.target.database.push_str("_other");
        assert!(apply_business_adoption(&database, &changed, &[&initial]).await.unwrap_err().to_string().contains("摘要不一致"));
        let next = next_migration();
        assert!(apply_business_adoption(&database, &plan, &[&initial, &next]).await.unwrap_err().to_string().contains("当前编译"));
        mysql::execute(&database, "ALTER TABLE biz_sql_example MODIFY code VARCHAR(64) NOT NULL").await?;
        let drift = apply_business_adoption(&database, &plan, &[&initial]).await.unwrap_err().to_string();
        assert!(drift.contains("结构不匹配") && drift.contains("code"));
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'", 0).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_rejects_another_physical_database_even_with_matching_schema() {
    mysql::run_mysql_test("adopttarget", |database| async move {
        mysql::execute(&database, DDL).await?;
        let initial = migration(false);
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial]).await.map_err(message)?;
        mysql::run_mysql_test("adoptwrongtarget", |other| async move {
            mysql::execute(&other, DDL).await?;
            assert!(apply_business_adoption(&other, &plan, &[&initial]).await.unwrap_err().to_string().contains("物理实例或数据库"));
            mysql::require_count(&other, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'", 0).await?;
            Ok(())
        }).await;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_registers_complete_prefix_without_executing_historical_ddl() {
    mysql::run_mysql_test("adoptprefix", |database| async move {
        existing_latest(&database).await?;
        let initial = migration(false);
        let next = next_migration();
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial, &next]).await.map_err(message)?;
        assert_eq!(plan.manifest.tables.len(), 1);
        assert_eq!(apply_business_adoption(&database, &plan, &[&initial, &next]).await.map_err(message)?.registered, 2);
        verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial, &next]).await.map_err(message)?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration WHERE origin='adopted'", 2).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_sql_example WHERE code='preserve-prefix' AND memo='existing memo'", 1).await?;
        mysql::execute(&database, "DELETE FROM ryframe_business_migration WHERE migration_id='m20261014_000001_example'").await?;
        assert!(inspect_business_adoption(&database, &plan, &[&initial, &next]).await.unwrap_err().to_string().contains("部分计划记录"));
        assert!(apply_business_adoption(&database, &plan, &[&initial, &next]).await.is_err());
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 1).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adoption_rolls_back_the_entire_batch_on_second_insert_failure() {
    mysql::run_mysql_test("adoptrollback", |database| async move {
        existing_latest(&database).await?;
        empty_ledger(&database).await?;
        let initial = migration(false);
        let next = next_migration();
        let plan = preview_business_adoption(&database, BusinessMigrationScope::Control, initial.module(), &[&initial, &next]).await.map_err(message)?;
        let before = inventory(&database).await?;
        let rows = business_rows(&database).await?;
        database.execute_unprepared("CREATE TRIGGER reject_second_adoption BEFORE INSERT ON ryframe_business_migration FOR EACH ROW BEGIN IF NEW.migration_id='m20261014_000001_example' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='fixture second record rejected'; END IF; END").await.map_err(mysql::db_error)?;
        let failure = apply_business_adoption(&database, &plan, &[&initial, &next]).await.unwrap_err().to_string();
        assert!(failure.contains("fixture second record rejected") && failure.contains("事务已回滚"));
        assert_eq!(inventory(&database).await?, before);
        assert_eq!(business_rows(&database).await?, rows);
        assert_eq!(inspect_business_adoption(&database, &plan, &[&initial, &next]).await.map_err(message)?, BusinessAdoptionState::Unapplied);
        database.execute_unprepared("DROP TRIGGER reject_second_adoption").await.map_err(mysql::db_error)?;
        assert_eq!(apply_business_adoption(&database, &plan, &[&initial, &next]).await.map_err(message)?.registered, 2);
        Ok(())
    }).await;
}

async fn existing_latest(database: &DatabaseConnection) -> Result<(), String> {
    mysql::execute(database, DDL).await?;
    mysql::execute(
        database,
        "ALTER TABLE biz_sql_example ADD memo VARCHAR(32) NULL",
    )
    .await?;
    mysql::execute(
        database,
        "INSERT INTO biz_sql_example(code,memo) VALUES('preserve-prefix','existing memo')",
    )
    .await
}

pub(super) async fn empty_ledger(database: &DatabaseConnection) -> Result<(), String> {
    let initial = migration(false).with_initialization_sql(&[DDL]);
    let statements = business_initialization_statements(&[&initial]).map_err(message)?;
    mysql::execute(database, &statements[0]).await
}

pub(super) async fn timestamps(database: &DatabaseConnection) -> Result<Vec<String>, String> {
    database.query_all_raw(Statement::from_string(DbBackend::MySql, "SELECT CAST(UNIX_TIMESTAMP(applied_at) AS CHAR) AS value FROM ryframe_business_migration ORDER BY scope,module_name,migration_id"))
        .await.map_err(mysql::db_error)?.into_iter().map(|row| row.try_get("", "value").map_err(mysql::db_error)).collect()
}

pub(super) fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

async fn access_rows(database: &DatabaseConnection) -> Result<Vec<String>, String> {
    let mut rows = Vec::new();
    for sql in [
        "SELECT CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'name',name,'code',code,'parent_id',parent_id,'perm_type',perm_type,'icon',icon,'sort',sort,'status',status,'created_at',created_at,'updated_at',updated_at) AS CHAR) AS item FROM sys_permission ORDER BY id",
        "SELECT CAST(JSON_OBJECT('id',id,'tenant_id',tenant_id,'name',name,'parent_id',parent_id,'menu_type',menu_type,'perm_id',perm_id,'route_key',route_key,'icon',icon,'sort',sort,'visible',visible,'status',status,'remark',remark,'del_flag',del_flag,'created_at',created_at,'updated_at',updated_at) AS CHAR) AS item FROM sys_menu ORDER BY id",
    ] {
        let result = database
            .query_all_raw(Statement::from_string(DbBackend::MySql, sql))
            .await
            .map_err(mysql::db_error)?;
        rows.extend(
            result
                .into_iter()
                .map(|row| row.try_get::<String>("", "item").map_err(mysql::db_error))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(rows)
}
