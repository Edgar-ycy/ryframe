use ryframe_sdk::persistence::sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};
use ryframe_sdk::{
    BusinessMigration, BusinessMigrationSchema, BusinessMigrationScope, BusinessTableSchema,
    SeaOrmBusinessMigration, business_initialization_statements, verify_business_migrations,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;

const DDL: &str = "CREATE TABLE biz_sql_example (id BIGINT NOT NULL AUTO_INCREMENT, code VARCHAR(32) NOT NULL, PRIMARY KEY(id), UNIQUE KEY uk_example_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci";
const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TABLE_SCHEMA: &str = r#"v2|table="biz_sql_example"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["id":"bigint":"NO":None:None:"PRI":None:"auto_increment":"";"code":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"UNI":None:"":"";]|indexes=["PRIMARY":"id":1:0:"btree":None:"YES";"uk_example_code":"code":1:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";"uk_example_code":"UNIQUE":"YES";]|checks=[]|foreign_keys=[]"#;
const AFTER_SCHEMA: &str = r#"v2|table="biz_sql_example"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["id":"bigint":"NO":None:None:"PRI":None:"auto_increment":"";"code":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"UNI":None:"":"";"memo":"varchar(32)":"YES":Some("utf8mb4"):Some("utf8mb4_general_ci"):"":None:"":"";]|indexes=["PRIMARY":"id":1:0:"btree":None:"YES";"uk_example_code":"code":1:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";"uk_example_code":"UNIQUE":"YES";]|checks=[]|foreign_keys=[]"#;
const NEXT_FINGERPRINT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Initial {
    mismatched: bool,
}
impl MigrationName for Initial {
    fn name(&self) -> &str {
        "m20261014_000000_example"
    }
}

#[ryframe_sdk::async_trait]
impl MigrationTrait for Initial {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let ddl = if self.mismatched {
            DDL.replace("VARCHAR(32)", "VARCHAR(64)")
        } else {
            DDL.into()
        };
        manager.get_connection().execute_unprepared(&ddl).await?;
        Ok(())
    }
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("测试不执行 down".into()))
    }
}

fn migration(mismatched: bool) -> SeaOrmBusinessMigration {
    // 验证元数据中的引号、反斜杠与中文不受客户端 SQL 模式影响。
    SeaOrmBusinessMigration::new(
        "sales'\\数据",
        "m20261014_000000_example",
        CHECKSUM,
        BusinessMigrationSchema {
            before_fingerprint: None,
            after_fingerprint: CHECKSUM,
            tables: &[BusinessTableSchema {
                table: "biz_sql_example",
                before: None,
                after: TABLE_SCHEMA,
            }],
        },
        BusinessMigrationScope::Control,
        Initial { mismatched },
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
    let missing = migration(false);
    let error = business_initialization_statements(&[&missing])
        .unwrap_err()
        .to_string();
    assert!(error.contains("control up") && error.contains("尚未写入"));
    let empty = migration(false).with_initialization_sql(&[]);
    assert!(
        business_initialization_statements(&[&empty])
            .unwrap_err()
            .to_string()
            .contains("SQL 为空")
    );
    let valid = migration(false).with_initialization_sql(&[DDL]);
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
        let migration = migration(false).with_initialization_sql(&[DDL]);
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
        verify_business_migrations(&imported, &[&migration])
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
            verify_business_migrations(&migrated, &[&migration])
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
        let broken = migration(false).with_initialization_sql(&[DDL, "CREATE TABLE broken syntax"]);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_schema_drift_and_ledger_fingerprint_tampering_fail_closed() {
    mysql::run_mysql_test("businessdrift", |database| async move {
        let migration = migration(false);
        migration.up(&database).await.map_err(|error| error.to_string())?;
        verify_business_migrations(&database, &[&migration]).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "ALTER TABLE biz_sql_example MODIFY code VARCHAR(64) NOT NULL DEFAULT 'do-not-print-this-value'").await?;
        let error = verify_business_migrations(&database, &[&migration]).await.unwrap_err().to_string();
        assert!(error.contains("列 code") && error.contains("类型") && error.contains("默认值") && error.contains("SHOW CREATE TABLE"), "{error}");
        assert!(!error.contains("do-not-print-this-value"));
        mysql::execute(&database, "UPDATE ryframe_business_migration SET after_fingerprint=REPEAT('b',64)").await?;
        let ledger_error = migration.status(&database).await.unwrap_err().to_string();
        assert!(ledger_error.contains("账本不一致") && ledger_error.contains("after_fingerprint"));
        assert!(migration.up(&database).await.unwrap_err().to_string().contains("账本不一致"));
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 1).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_table_preflight_and_mismatched_ddl_never_claim_success() {
    mysql::run_mysql_test("businesspreexisting", |database| async move {
        mysql::execute(&database, DDL).await?;
        let error = migration(false).up(&database).await.unwrap_err().to_string();
        assert!(error.contains("已存在") && error.contains("尚未执行"));
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'", 0).await?;
        Ok(())
    }).await;
    mysql::run_mysql_test("businessmismatch", |database| async move {
        let migration = migration(true);
        let error = migration.up(&database).await.unwrap_err().to_string();
        assert!(error.contains("列 code") && error.contains("未登记本次账本"));
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='biz_sql_example' AND column_name='code' AND column_type='varchar(64)'", 1).await?;
        assert!(migration.up(&database).await.unwrap_err().to_string().contains("已存在"));
        Ok(())
    }).await;
}

struct Next;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ledger_write_failure_preserves_ddl_and_requires_authoritative_recheck() {
    mysql::run_mysql_test("businessreceipt", |database| async move {
        let migration = migration(false).with_initialization_sql(&[DDL]);
        let statements = business_initialization_statements(&[&migration]).map_err(|error| error.to_string())?;
        mysql::execute(&database, &statements[0]).await?;
        database.execute_unprepared("CREATE TRIGGER deny_business_receipt BEFORE INSERT ON ryframe_business_migration FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='fixture denied ledger write'").await.map_err(mysql::db_error)?;
        let error = migration.up(&database).await.unwrap_err().to_string();
        assert!(error.contains("DDL 已执行并通过结构校验") && error.contains("账本写入失败") && error.contains("不自动重放"), "{error}");
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='biz_sql_example'", 1).await?;
        assert!(migration.up(&database).await.unwrap_err().to_string().contains("已存在"));
        Ok(())
    }).await;
}

impl MigrationName for Next {
    fn name(&self) -> &str {
        "m20261014_000001_example"
    }
}

#[ryframe_sdk::async_trait]
impl MigrationTrait for Next {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE biz_sql_example ADD memo VARCHAR(32) NULL")
            .await?;
        Ok(())
    }
    async fn down(&self, _: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("测试只执行向前迁移".into()))
    }
}

fn next_migration() -> SeaOrmBusinessMigration {
    SeaOrmBusinessMigration::new(
        migration(false).module(),
        "m20261014_000001_example",
        NEXT_FINGERPRINT,
        BusinessMigrationSchema {
            before_fingerprint: Some(CHECKSUM),
            after_fingerprint: NEXT_FINGERPRINT,
            tables: &[BusinessTableSchema {
                table: "biz_sql_example",
                before: Some(TABLE_SCHEMA),
                after: AFTER_SCHEMA,
            }],
        },
        BusinessMigrationScope::Control,
        Next,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_table_contract_accepts_roll_forward_without_rechecking_historical_shape() {
    mysql::run_mysql_test("businesslatest", |database| async move {
        let initial = migration(false);
        let next = next_migration();
        initial
            .up(&database)
            .await
            .map_err(|error| error.to_string())?;
        mysql::execute(
            &database,
            "INSERT INTO biz_sql_example(code) VALUES ('keep-data')",
        )
        .await?;
        next.up(&database)
            .await
            .map_err(|error| error.to_string())?;
        verify_business_migrations(&database, &[&initial, &next])
            .await
            .map_err(|error| error.to_string())?;
        initial
            .verify_ledger(&database)
            .await
            .map_err(|error| error.to_string())?;
        assert!(
            verify_business_migrations(&database, &[&next, &initial])
                .await
                .unwrap_err()
                .to_string()
                .contains("前后结构链")
        );
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM biz_sql_example WHERE code='keep-data' AND memo IS NULL",
            1,
        )
        .await?;
        mysql::require_count(
            &database,
            "SELECT COUNT(*) AS value FROM ryframe_business_migration",
            2,
        )
        .await?;
        Ok(())
    })
    .await;
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
