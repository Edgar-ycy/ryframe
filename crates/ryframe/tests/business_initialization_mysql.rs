use ryframe_sdk::persistence::sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};
use ryframe_sdk::{
    BusinessMigration, BusinessMigrationScope, SeaOrmBusinessMigration,
    business_initialization_statements,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;

const DDL: &str = "CREATE TABLE biz_sql_example (id BIGINT NOT NULL AUTO_INCREMENT, code VARCHAR(32) NOT NULL, PRIMARY KEY(id), UNIQUE KEY uk_example_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci";
const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Initial;
impl MigrationName for Initial {
    fn name(&self) -> &str {
        "m20261014_000000_example"
    }
}

#[ryframe_sdk::async_trait]
impl MigrationTrait for Initial {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DDL).await?;
        Ok(())
    }
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("测试不执行 down".into()))
    }
}

fn migration() -> SeaOrmBusinessMigration {
    // 验证元数据中的引号、反斜杠与中文不受客户端 SQL 模式影响。
    SeaOrmBusinessMigration::new(
        "sales'\\数据",
        "m20261014_000000_example",
        CHECKSUM,
        None,
        CHECKSUM,
        BusinessMigrationScope::Control,
        Initial,
    )
}

fn hashes() -> Result<ryframe_db::migration::BootstrapHashes, String> {
    let admin = ryframe_auth::password::hash("Integration123!initial")
        .map_err(|error| error.to_string())?;
    let user = ryframe_auth::password::hash("123456").map_err(|error| error.to_string())?;
    ryframe_db::migration::BootstrapHashes::new(admin, user).map_err(mysql::db_error)
}

#[test]
fn missing_empty_and_duplicate_sql_descriptions_fail_before_export() {
    let missing = migration();
    let error = business_initialization_statements(&[&missing])
        .unwrap_err()
        .to_string();
    assert!(error.contains("control up") && error.contains("尚未写入"));
    let empty = migration().with_initialization_sql(&[]);
    assert!(
        business_initialization_statements(&[&empty])
            .unwrap_err()
            .to_string()
            .contains("SQL 为空")
    );
    let valid = migration().with_initialization_sql(&[DDL]);
    assert!(
        business_initialization_statements(&[&valid, &valid])
            .unwrap_err()
            .to_string()
            .contains("重复登记")
    );
    assert!(business_initialization_statements(&[]).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_sql_and_up_keep_business_schema_and_ledger_equivalent() {
    ryframe_db::migration::register_business_tables(["biz_sql_example"]).unwrap();
    mysql::run_mysql_test("businesssql", |imported| async move {
        let migration = migration().with_initialization_sql(&[DDL]);
        let statements =
            business_initialization_statements(&[&migration]).map_err(|error| error.to_string())?;
        let sql =
            ryframe_db::migration::mysql_initialization_sql_with_business(&hashes()?, &statements);
        imported
            .execute_unprepared(&sql)
            .await
            .map_err(mysql::db_error)?;
        ryframe_db::migration::verify(&imported)
            .await
            .map_err(mysql::db_error)?;
        migration
            .verify(&imported)
            .await
            .map_err(|error| error.to_string())?;
        let expected = inventory(&imported).await?;
        mysql::run_mysql_test("businessup", |migrated| async move {
            ryframe_db::migration::up(&migrated, Some(&hashes()?))
                .await
                .map_err(mysql::db_error)?;
            migration
                .up(&migrated)
                .await
                .map_err(|error| error.to_string())?;
            migration
                .verify(&migrated)
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(expected, inventory(&migrated).await?);
            migration
                .up(&migrated)
                .await
                .map_err(|error| error.to_string())?;
            mysql::require_count(
                &migrated,
                "SELECT COUNT(*) AS value FROM ryframe_business_migration",
                1,
            )
            .await?;
            Ok(())
        })
        .await;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_business_sql_cannot_register_control_completion() {
    mysql::run_mysql_test("businessfail", |database| async move {
        let broken = migration().with_initialization_sql(&[DDL, "CREATE TABLE broken syntax"]);
        let statements = business_initialization_statements(&[&broken]).map_err(|error| error.to_string())?;
        let sql = ryframe_db::migration::mysql_initialization_sql_with_business(&hashes()?, &statements);
        assert!(database.execute_unprepared(&sql).await.is_err());
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='biz_sql_example'", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='seaql_migrations'", 0).await?;
        assert!(ryframe_db::migration::verify(&database).await.is_err());
        Ok(())
    }).await;
}

async fn inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CONCAT_WS('|', TABLE_NAME,COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COALESCE(COLUMN_DEFAULT,'NULL'),EXTRA) AS item FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name IN ('biz_sql_example','ryframe_business_migration') ORDER BY TABLE_NAME,ORDINAL_POSITION",
        "SELECT CONCAT_WS('|', TABLE_NAME,INDEX_NAME,NON_UNIQUE,SEQ_IN_INDEX,COLUMN_NAME) AS item FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name IN ('biz_sql_example','ryframe_business_migration') ORDER BY TABLE_NAME,INDEX_NAME,SEQ_IN_INDEX",
        "SELECT CONCAT_WS('|', module_name,migration_id,checksum,COALESCE(before_fingerprint,'NULL'),after_fingerprint) AS item FROM ryframe_business_migration ORDER BY module_name,migration_id",
    ] {
        let rows = database
            .query_all_raw(Statement::from_string(DbBackend::MySql, sql))
            .await
            .map_err(mysql::db_error)?;
        result.push(
            rows.into_iter()
                .map(|row| row.try_get("", "item").map_err(mysql::db_error))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(result)
}
