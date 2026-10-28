use ryframe_sdk::persistence::sea_orm_migration::{MigrationName, MigrationTrait, SchemaManager};
use ryframe_sdk::{
    BusinessMigration, BusinessMigrationSchema, BusinessMigrationScope, BusinessTableSchema,
    SeaOrmBusinessMigration, business_initialization_statements, verify_business_migrations,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait};

#[path = "../../ryframe-db/tests/support/mysql.rs"]
mod mysql;

#[path = "business_initialization_mysql/adoption.rs"]
mod adoption;

#[path = "business_initialization_mysql/adoption_races.rs"]
mod adoption_races;
#[path = "business_initialization_mysql/adoption_unknown.rs"]
mod adoption_unknown;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;
#[path = "../src/bin/ryframe_migrate/diagnostic.rs"]
mod diagnostic;
#[path = "../src/bin/ryframe_migrate/ownership/guard.rs"]
mod migration_guard;
#[path = "../src/bin/ryframe_migrate/targets.rs"]
mod migration_targets;
#[path = "business_initialization_mysql/ownership.rs"]
mod ownership;
#[cfg(feature = "bin-api")]
#[path = "business_initialization_mysql/replicas.rs"]
mod replicas;
#[path = "business_initialization_mysql/response_gate.rs"]
mod response_gate;
#[path = "business_initialization_mysql/sql_guard.rs"]
mod sql_guard;
#[path = "business_initialization_mysql/target_identity.rs"]
mod target_identity;
use sql_guard::{assert_schema_predicates, verify_non_check_value_boundaries};

const DDL: &str = "CREATE TABLE biz_sql_example (id BIGINT NOT NULL AUTO_INCREMENT, code VARCHAR(32) NOT NULL, PRIMARY KEY(id), UNIQUE KEY uk_example_code(code)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci";
const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TABLE_SCHEMA: &str = r#"v2|table="biz_sql_example"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["id":"bigint":"NO":None:None:"PRI":None:"auto_increment":"";"code":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"UNI":None:"":"";]|indexes=["PRIMARY":"id":1:0:"btree":None:"YES";"uk_example_code":"code":1:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";"uk_example_code":"UNIQUE":"YES";]|checks=[]|foreign_keys=[]"#;
const AFTER_SCHEMA: &str = r#"v2|table="biz_sql_example"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["id":"bigint":"NO":None:None:"PRI":None:"auto_increment":"";"code":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"UNI":None:"":"";"memo":"varchar(32)":"YES":Some("utf8mb4"):Some("utf8mb4_general_ci"):"":None:"":"";]|indexes=["PRIMARY":"id":1:0:"btree":None:"YES";"uk_example_code":"code":1:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";"uk_example_code":"UNIQUE":"YES";]|checks=[]|foreign_keys=[]"#;
const NEXT_FINGERPRINT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const TENANT_DDL: &str = "CREATE TABLE biz_tenant_example (tenant_id VARCHAR(32) NOT NULL, id BIGINT NOT NULL, PRIMARY KEY(tenant_id,id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci";
const TENANT_SCHEMA: &str = r#"v2|table="biz_tenant_example"|engine="innodb"|charset="utf8mb4"|collation="utf8mb4_general_ci"|columns=["tenant_id":"varchar(32)":"NO":Some("utf8mb4"):Some("utf8mb4_general_ci"):"PRI":None:"":"";"id":"bigint":"NO":None:None:"PRI":None:"":"";]|indexes=["PRIMARY":"tenant_id":1:0:"btree":None:"YES";"PRIMARY":"id":2:0:"btree":None:"YES";]|constraints=["PRIMARY":"PRIMARY KEY":"YES";]|checks=[]|foreign_keys=[]"#;
const TENANT_TABLE: ryframe_application::ports::tenant_data::TenantDataTableDescriptor =
    ryframe_application::ports::tenant_data::TenantDataTableDescriptor {
        table: "biz_tenant_example",
        copy_order: 1,
        tenant_column: "tenant_id",
        primary_key_cursor_columns: &["tenant_id", "id"],
        checksum_columns: &["tenant_id", "id"],
        column_types: &["varchar", "bigint"],
        has_generated_columns: false,
        foreign_key_dependencies: &[],
        foreign_keys: &[],
        schema_canonical: TENANT_SCHEMA,
    };

struct TenantInitial;
impl MigrationName for TenantInitial {
    fn name(&self) -> &str {
        "m20261014_000000_tenant_example"
    }
}
#[ryframe_sdk::async_trait]
impl MigrationTrait for TenantInitial {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(TENANT_DDL)
            .await?;
        Ok(())
    }
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("测试不执行 down".into()))
    }
}
fn tenant_migration() -> SeaOrmBusinessMigration {
    SeaOrmBusinessMigration::new(
        "tenant-example",
        "m20261014_000000_tenant_example",
        CHECKSUM,
        BusinessMigrationSchema {
            before_fingerprint: None,
            after_fingerprint: CHECKSUM,
            tables: &[BusinessTableSchema {
                table: "biz_tenant_example",
                before: None,
                after: TENANT_SCHEMA,
            }],
        },
        BusinessMigrationScope::Tenant,
        TenantInitial,
    )
    .with_initialization_sql(&[TENANT_DDL])
}

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
    let error = business_initialization_statements(&[&missing], &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("control up") && error.contains("尚未写入"));
    let empty = migration(false).with_initialization_sql(&[]);
    assert!(
        business_initialization_statements(&[&empty], &[])
            .unwrap_err()
            .to_string()
            .contains("SQL 为空")
    );
    let valid = migration(false).with_initialization_sql(&[DDL]);
    assert!(
        business_initialization_statements(&[&valid, &valid], &[])
            .unwrap_err()
            .to_string()
            .contains("重复登记")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_receipt_holds_table_metadata_lock_until_commit() {
    mysql::run_mysql_test("businessmetagate", |database| async move {
        migration(false).up(&database).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "INSERT INTO biz_sql_example(code) VALUES ('keep-metadata-data')").await?;
        let before = business_rows(&database).await?;
        mysql::execute(&database, "CREATE TABLE fixture_ledger_gate (id BIGINT PRIMARY KEY)").await?;
        mysql::execute(&database, "INSERT INTO fixture_ledger_gate VALUES(1)").await?;
        database.execute_unprepared("CREATE TRIGGER hold_business_receipt BEFORE INSERT ON ryframe_business_migration FOR EACH ROW BEGIN DECLARE gate_id BIGINT; SELECT id INTO gate_id FROM fixture_ledger_gate WHERE id=1 FOR UPDATE; END").await.map_err(mysql::db_error)?;
        let gate = database.begin().await.map_err(mysql::db_error)?;
        gate.query_one_raw(Statement::from_string(DbBackend::MySql, "SELECT id FROM fixture_ledger_gate WHERE id=1 FOR UPDATE")).await.map_err(mysql::db_error)?;
        let write_connection = database.clone();
        let writing = tokio::spawn(async move { next_migration().up(&write_connection).await.map_err(|error| error.to_string()) });
        let blocked = wait_for_database_state(&database, "SELECT COUNT(*) AS value FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID AND l.ENGINE=w.ENGINE WHERE l.OBJECT_SCHEMA=DATABASE() AND l.OBJECT_NAME='fixture_ledger_gate'", "账本 INSERT 被测试 gate 阻塞").await;
        if let Err(error) = blocked {
            gate.rollback().await.map_err(mysql::db_error)?;
            writing.await.map_err(|error| error.to_string())??;
            return Err(error);
        }
        let ddl_connection = database.clone();
        let altering = tokio::spawn(async move { mysql::execute(&ddl_connection, "ALTER TABLE biz_sql_example ADD note VARCHAR(32) NULL").await });
        let waiting = wait_for_database_state(&database, "SELECT COUNT(*) AS value FROM performance_schema.metadata_locks WHERE OBJECT_SCHEMA=DATABASE() AND OBJECT_NAME='biz_sql_example' AND LOCK_STATUS='PENDING'", "外部 ALTER 等待业务表 MDL").await;
        gate.rollback().await.map_err(mysql::db_error)?;
        let (write_result, ddl_result) = tokio::join!(writing, altering);
        write_result.map_err(|error| error.to_string())??;
        ddl_result.map_err(|error| error.to_string())??;
        waiting?;
        assert_eq!(business_rows(&database).await?, before);
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration WHERE origin='executed'", 2).await?;
        let initial = migration(false);
        let next = next_migration();
        let drift = verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial, &next]).await.unwrap_err().to_string();
        assert!(drift.contains("note"));
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_ledger_rejects_unknown_records_and_foreign_table_ownership() {
    mysql::run_mysql_test("businessscope", |database| async move {
        let initial = migration(false);
        initial.up(&database).await.map_err(|error| error.to_string())?;
        assert_eq!(initial.status(&database).await.unwrap().origin, Some(ryframe_sdk::BusinessMigrationOrigin::Executed));
        verify_business_migrations(&database, BusinessMigrationScope::Tenant, &[]).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "INSERT INTO ryframe_business_migration(scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash) SELECT 'tenant','other','other',checksum,before_fingerprint,after_fingerprint,origin,JSON_ARRAY('biz_other'),adoption_hash FROM ryframe_business_migration WHERE scope='control'").await?;
        verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial]).await.map_err(|error| error.to_string())?;
        let unknown = verify_business_migrations(&database, BusinessMigrationScope::Tenant, &[]).await.unwrap_err().to_string();
        assert!(unknown.contains("未注册") && unknown.contains("other/other"));
        let before = business_rows(&database).await?;
        mysql::execute(&database, "UPDATE ryframe_business_migration SET tables_json=JSON_ARRAY('biz_sql_example') WHERE scope='tenant'").await?;
        let ownership = verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial]).await.unwrap_err().to_string();
        assert!(ownership.contains("已登记归属 tenant/other"));
        assert!(initial.up(&database).await.unwrap_err().to_string().contains("已登记归属"));
        assert!(initial.status(&database).await.unwrap_err().to_string().contains("已登记归属"));
        assert!(initial.verify_ledger(&database).await.unwrap_err().to_string().contains("已登记归属"));
        let next = next_migration();
        let error = next.up(&database).await.unwrap_err().to_string();
        assert!(error.contains("已登记归属"));
        assert_eq!(business_rows(&database).await?, before);
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='biz_sql_example' AND column_name='memo'", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 2).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ledger_table_lists_and_origin_hashes_are_verified_without_guessing() {
    mysql::run_mysql_test("businessorigin", |database| async move {
        let initial = migration(false);
        initial.up(&database).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "UPDATE ryframe_business_migration SET tables_json=JSON_ARRAY('biz_other')").await?;
        assert!(initial.status(&database).await.unwrap_err().to_string().contains("tables_json"));
        mysql::execute(&database, "UPDATE ryframe_business_migration SET tables_json=JSON_ARRAY('biz_sql_example'), origin='adopted', adoption_hash=REPEAT('z',64)").await?;
        assert!(initial.status(&database).await.unwrap_err().to_string().contains("origin/adoption_hash"));
        mysql::execute(&database, "UPDATE ryframe_business_migration SET adoption_hash=REPEAT('a',64)").await?;
        assert_eq!(initial.status(&database).await.unwrap().origin, Some(ryframe_sdk::BusinessMigrationOrigin::Adopted));
        verify_business_migrations(&database, BusinessMigrationScope::Control, &[&initial]).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "UPDATE ryframe_business_migration SET scope='invalid'").await?;
        assert!(verify_business_migrations(&database, BusinessMigrationScope::Control, &[]).await.unwrap_err().to_string().contains("未注册"));
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 1).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_business_schema_verification_is_read_only_and_rejects_drift() {
    use ryframe_db::migration::{register_business_tables, verify_business_schema};
    register_business_tables([("biz_sql_example", TABLE_SCHEMA)]).unwrap();
    mysql::run_mysql_test("businessruntime", |database| async move {
        let missing = verify_business_schema(&database).await.unwrap_err().to_string();
        assert!(missing.contains("biz_sql_example") && missing.contains("control verify"));
        let initial = ryframe_db::schema::schema_preconditions("biz_sql_example", None).map_err(mysql::db_error)?;
        assert_schema_predicates(&database, &initial, &[]).await?;
        mysql::execute(&database, DDL).await?;
        let after = ryframe_db::schema::schema_assertions("biz_sql_example", TABLE_SCHEMA).map_err(mysql::db_error)?;
        assert_schema_predicates(&database, &after, &[]).await?;
        assert_schema_predicates(&database, &initial, &["initial_absent"]).await?;
        verify_business_schema(&database).await.map_err(mysql::db_error)?;
        assert!(register_business_tables([("biz_sql_example", AFTER_SCHEMA), ("invalid_table", "invalid")]).is_err());
        assert!(register_business_tables([("biz_sql_example", TABLE_SCHEMA), ("biz_sql_example", TABLE_SCHEMA)]).is_err());
        assert!(register_business_tables([("sys_user", TABLE_SCHEMA)]).is_err());
        verify_business_schema(&database).await.map_err(mysql::db_error)?;
        mysql::execute(&database, "INSERT INTO biz_sql_example(code) VALUES ('keep-runtime-data')").await?;
        mysql::execute(&database, "ALTER TABLE biz_sql_example MODIFY code VARCHAR(64) NOT NULL DEFAULT 'do-not-print-this-value'").await?;
        let before = (
            ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example").await.map_err(mysql::db_error)?,
            business_rows(&database).await?,
        );
        assert_schema_predicates(&database, &after, &["columns"]).await?;
        let drift = verify_business_schema(&database).await.unwrap_err().to_string();
        assert!(drift.contains("biz_sql_example") && drift.contains("code") && drift.contains("默认值"));
        assert!(!drift.contains("do-not-print-this-value"));
        assert_eq!((
            ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example").await.map_err(mysql::db_error)?,
            business_rows(&database).await?,
        ), before);
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM biz_sql_example WHERE code='keep-runtime-data'", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='ryframe_business_migration'", 0).await?;
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_sql_and_up_keep_business_schema_and_ledger_equivalent() {
    ryframe_db::migration::register_business_tables([("biz_sql_example", TABLE_SCHEMA)]).unwrap();
    ryframe_tenant_db::migration::register_business_catalog([TENANT_TABLE], ["biz_sql_example"])
        .unwrap();
    mysql::run_mysql_test("businesssql", |imported| async move {
        let migration = migration(false).with_initialization_sql(&[DDL]);
        let tenant = tenant_migration();
        let sql = ryframe::business::shared_control_initialization_sql(
            "initialization-ownership-test",
            &hashes()?,
            &[&migration, &tenant],
        )
        .map_err(|error| error.to_string())?;
        imported
            .execute_unprepared(&sql)
            .await
            .map_err(mysql::db_error)?;
        ryframe_db::migration::verify(&imported)
            .await
            .map_err(mysql::db_error)?;
        verify_business_migrations(&imported, BusinessMigrationScope::Control, &[&migration])
            .await
            .map_err(|error| error.to_string())?;
        ryframe_tenant_db::migration::verify_shared_control(&imported)
            .await
            .map_err(mysql::db_error)?;
        verify_business_migrations(&imported, BusinessMigrationScope::Tenant, &[&tenant])
            .await
            .map_err(|error| error.to_string())?;
        let expected = shared_inventory(&imported).await?;
        let nonempty = imported
            .execute_unprepared(&sql)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            nonempty.contains("ryframe_initialization_guard"),
            "{nonempty}"
        );
        assert_eq!(expected, shared_inventory(&imported).await?);
        mysql::run_mysql_test("businessup", |migrated| async move {
            ryframe_db::migration::up(&migrated, Some(&hashes()?))
                .await
                .map_err(mysql::db_error)?;
            ryframe_tenant_db::migration::up_pending_business_migrations(&migrated)
                .await
                .map_err(mysql::db_error)?;
            migrate_tenant_on_retained_session(&migrated, &tenant).await?;
            ryframe_tenant_db::migration::verify_shared_control(&migrated)
                .await
                .map_err(mysql::db_error)?;
            verify_business_migrations(&migrated, BusinessMigrationScope::Tenant, &[&tenant])
                .await
                .map_err(|error| error.to_string())?;
            migration
                .up(&migrated)
                .await
                .map_err(|error| error.to_string())?;
            verify_business_migrations(&migrated, BusinessMigrationScope::Control, &[&migration])
                .await
                .map_err(|error| error.to_string())?;
            for kind in ["control", "tenant-data"] {
                ryframe_db::resource_ownership::ensure_resource_ownership(
                    &migrated,
                    "initialization-ownership-test",
                    kind,
                )
                .await
                .map_err(mysql::db_error)?;
            }
            assert_eq!(expected, shared_inventory(&migrated).await?);
            migration
                .up(&migrated)
                .await
                .map_err(|error| error.to_string())?;
            mysql::require_count(
                &migrated,
                "SELECT COUNT(*) AS value FROM ryframe_business_migration",
                2,
            )
            .await?;
            verify_shared_business_boundary(&migrated).await?;
            Ok(())
        })
        .await;
        Ok(())
    })
    .await;
}

async fn migrate_tenant_on_retained_session(
    database: &sea_orm::DatabaseConnection,
    tenant: &dyn BusinessMigration,
) -> Result<(), String> {
    let mut retained = migration_guard::Guard::acquire(database)
        .await
        .map_err(mysql::db_error)?;
    assert!(tenant.up_in_session(retained.session_mut()).await.is_err());
    retained
        .session_mut()
        .finish_mysql_root(false)
        .await
        .map_err(mysql::db_error)?;
    tenant
        .up_in_session(retained.session_mut())
        .await
        .map_err(|error| error.to_string())?;
    ryframe_sdk::verify_business_migrations_in_session(
        retained.session(),
        BusinessMigrationScope::Tenant,
        &[tenant],
    )
    .await
    .map_err(|error| error.to_string())?;
    retained.close().await.map_err(mysql::db_error)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_business_sql_cannot_register_control_completion() {
    mysql::run_mysql_test("businessfail", |database| async move {
        let broken = migration(false).with_initialization_sql(&[DDL, "CREATE TABLE broken syntax"]);
        let sql = ryframe::business::shared_control_initialization_sql("initialization-ownership-test", &hashes()?, &[&broken]).map_err(|error| error.to_string())?;
        assert!(database.execute_unprepared(&sql).await.is_err());
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='biz_sql_example'", 1).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM seaql_migrations", 0).await?;
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM seaql_tenant_data_migrations", 0).await?;
        assert!(ryframe_db::migration::verify(&database).await.is_err());
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_schema_drift_and_ledger_fingerprint_tampering_fail_closed() {
    mysql::run_mysql_test("businessdrift", |database| async move {
        let migration = migration(false);
        migration.up(&database).await.map_err(|error| error.to_string())?;
        verify_business_migrations(&database, BusinessMigrationScope::Control, &[&migration]).await.map_err(|error| error.to_string())?;
        mysql::execute(&database, "ALTER TABLE biz_sql_example MODIFY code VARCHAR(64) NOT NULL DEFAULT 'do-not-print-this-value'").await?;
        let error = verify_business_migrations(&database, BusinessMigrationScope::Control, &[&migration]).await.unwrap_err().to_string();
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
        let before = ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example").await.map_err(mysql::db_error)?;
        // 编码 DDL 执行前即拒绝已有目标，不能用 IF NOT EXISTS 把原结构当作导入成功。
        let encoded = migration(false).with_initialization_sql(&[
            r#"CREATE TABLE IF NOT EXISTS `biz_sql_example` (`id` BIGINT) COMMENT='fixture\\path'"#
        ]);
        let statements = business_initialization_statements(&[&encoded], &[]).map_err(|error| error.to_string())?;
        let error = database.execute_unprepared(&statements.into_statements().join(";\n")).await.unwrap_err().to_string();
        assert!(error.contains("ryframe_schema_assertion_failed"), "{error}");
        assert_eq!(ryframe_db::schema::canonical_table_schema(&database, "biz_sql_example").await.map_err(mysql::db_error)?, before);
        mysql::require_count(&database, "SELECT COUNT(*) AS value FROM ryframe_business_migration", 0).await?;
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
        let statements = business_initialization_statements(&[&migration], &[]).map_err(|error| error.to_string())?;
        let ledger = statements.setup.iter().find(|sql|sql.starts_with("CREATE TABLE ryframe_business_migration ")).ok_or("导出计划缺少正式业务账本定义")?;
        mysql::execute(&database, ledger).await?;
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
        let before_next =
            ryframe_db::schema::schema_preconditions("biz_sql_example", Some(TABLE_SCHEMA))
                .map_err(mysql::db_error)?;
        assert_schema_predicates(&database, &before_next, &[]).await?;
        next.up(&database)
            .await
            .map_err(|error| error.to_string())?;
        let after_next = ryframe_db::schema::schema_assertions("biz_sql_example", AFTER_SCHEMA)
            .map_err(mysql::db_error)?;
        assert_schema_predicates(&database, &after_next, &[]).await?;
        assert_schema_predicates(&database, &before_next, &["columns"]).await?;
        verify_business_migrations(
            &database,
            BusinessMigrationScope::Control,
            &[&initial, &next],
        )
        .await
        .map_err(|error| error.to_string())?;
        initial
            .verify_ledger(&database)
            .await
            .map_err(|error| error.to_string())?;
        assert!(
            verify_business_migrations(
                &database,
                BusinessMigrationScope::Control,
                &[&next, &initial]
            )
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
        verify_non_check_value_boundaries(&database).await?;
        Ok(())
    })
    .await;
}

async fn wait_for_database_state(
    database: &DatabaseConnection,
    sql: &str,
    stage: &str,
) -> Result<(), String> {
    for _ in 0..200 {
        let row = database
            .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
            .await
            .map_err(mysql::db_error)?
            .ok_or_else(|| format!("无法观察 {stage}"))?;
        let count: i64 = row.try_get("", "value").map_err(mysql::db_error)?;
        if count > 0 {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let rows = database.query_all_raw(Statement::from_string(DbBackend::MySql,
        "SELECT p.ID AS id, COALESCE(p.STATE,'') AS state, COALESCE(t.trx_state,'') AS transaction_state, COALESCE(LEFT(p.INFO,160),'') AS statement FROM information_schema.processlist p LEFT JOIN information_schema.innodb_trx t ON t.trx_mysql_thread_id=p.ID WHERE p.DB=DATABASE() ORDER BY p.ID"))
        .await.map_err(mysql::db_error)?;
    let mut observed = Vec::new();
    for row in rows {
        observed.push((
            row.try_get::<u64>("", "id").map_err(mysql::db_error)?,
            row.try_get::<String>("", "state")
                .map_err(mysql::db_error)?,
            row.try_get::<String>("", "transaction_state")
                .map_err(mysql::db_error)?,
            row.try_get::<String>("", "statement")
                .map_err(mysql::db_error)?,
        ));
    }
    let data_wait = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT COUNT(*) AS value FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID AND l.ENGINE=w.ENGINE WHERE l.OBJECT_SCHEMA=DATABASE() AND l.OBJECT_NAME='fixture_ledger_gate'"))
        .await.map_err(mysql::db_error)?.ok_or("无法读取测试 gate 数据锁等待")?;
    let data_wait: i64 = data_wait.try_get("", "value").map_err(mysql::db_error)?;
    Err(format!(
        "未观察到预期数据库阶段：{stage}；gate 数据锁等待={data_wait}；独占 schema 现场：{observed:?}"
    ))
}

async fn business_rows(database: &DatabaseConnection) -> Result<Vec<(i64, String)>, String> {
    database
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT id,code FROM biz_sql_example ORDER BY id",
        ))
        .await
        .map_err(mysql::db_error)?
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get("", "id").map_err(mysql::db_error)?,
                row.try_get("", "code").map_err(mysql::db_error)?,
            ))
        })
        .collect()
}

async fn inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    for sql in [
        "SELECT CONCAT_WS('|', TABLE_NAME,COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COALESCE(COLUMN_DEFAULT,'NULL'),EXTRA) AS item FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name IN ('biz_sql_example','ryframe_business_migration') ORDER BY TABLE_NAME,ORDINAL_POSITION",
        "SELECT CONCAT_WS('|', TABLE_NAME,INDEX_NAME,NON_UNIQUE,SEQ_IN_INDEX,COLUMN_NAME) AS item FROM information_schema.statistics WHERE table_schema=DATABASE() AND table_name IN ('biz_sql_example','ryframe_business_migration') ORDER BY TABLE_NAME,INDEX_NAME,SEQ_IN_INDEX",
        "SELECT CONCAT_WS('|', scope,module_name,migration_id,checksum,COALESCE(before_fingerprint,'NULL'),after_fingerprint,origin,CAST(tables_json AS CHAR),COALESCE(adoption_hash,'NULL')) AS item FROM ryframe_business_migration ORDER BY scope,module_name,migration_id",
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

async fn verify_shared_business_boundary(database: &DatabaseConnection) -> Result<(), String> {
    use ryframe_tenant_db::migration as tenant;

    tenant::up_pending_business_migrations(database)
        .await
        .map_err(mysql::db_error)?;
    tenant::verify_shared_control(database)
        .await
        .map_err(mysql::db_error)?;
    mysql::execute(
        database,
        "INSERT INTO biz_tenant_example(tenant_id,id) VALUES ('system',1)",
    )
    .await?;
    // 再次应用框架迁移后只读确认，不应重放已完成的 DDL 或改变业务数据。
    let before = inventory(database).await?;
    tenant::up_pending_business_migrations(database)
        .await
        .map_err(mysql::db_error)?;
    tenant::verify_shared_control(database)
        .await
        .map_err(mysql::db_error)?;
    assert_eq!(before, inventory(database).await?);
    mysql::require_count(
        database,
        "SELECT COUNT(*) AS value FROM biz_tenant_example WHERE tenant_id='system' AND id=1",
        1,
    )
    .await?;
    let dedicated = tenant::verify_mysql_target(database)
        .await
        .unwrap_err()
        .to_string();
    assert!(dedicated.contains("biz_sql_example") && dedicated.contains("未知表"));
    mysql::execute(database, "CREATE TABLE biz_unknown (id BIGINT PRIMARY KEY)").await?;
    let unknown = tenant::verify_shared_control(database)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        unknown.contains("biz_unknown")
            && unknown.contains("未知表")
            && unknown.contains("勿删库")
            && unknown.contains("此前迁移可能已完成 DDL")
    );
    mysql::execute(database, "DROP TABLE biz_unknown").await?;
    mysql::execute(database, "DROP TABLE biz_tenant_example").await?;
    let missing = tenant::verify_shared_control(database)
        .await
        .unwrap_err()
        .to_string();
    assert!(missing.contains("biz_tenant_example") && missing.contains("缺失表"));
    Ok(())
}

async fn shared_inventory(database: &DatabaseConnection) -> Result<Vec<Vec<String>>, String> {
    let mut result = inventory(database).await?;
    for sql in [
        "SELECT CONCAT_WS('|',TABLE_NAME,COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COALESCE(COLUMN_DEFAULT,'NULL'),EXTRA) AS item FROM information_schema.columns WHERE table_schema=DATABASE() AND (LEFT(table_name,4)='biz_' OR table_name='seaql_tenant_data_migrations') ORDER BY TABLE_NAME,ORDINAL_POSITION",
        "SELECT CONCAT_WS('|',TABLE_NAME,INDEX_NAME,NON_UNIQUE,SEQ_IN_INDEX,COLUMN_NAME) AS item FROM information_schema.statistics WHERE table_schema=DATABASE() AND (LEFT(table_name,4)='biz_' OR table_name='seaql_tenant_data_migrations') ORDER BY TABLE_NAME,INDEX_NAME,SEQ_IN_INDEX",
        "SELECT version AS item FROM seaql_tenant_data_migrations ORDER BY version",
        "SELECT CONCAT_WS('|',resource_kind,scope_id,marker) AS item FROM ryframe_resource_ownership ORDER BY resource_kind",
        "SELECT CONCAT_WS('|',tenant_id,target_key,placement_generation,state,switch_token) AS item FROM biz_tenant_fence ORDER BY tenant_id",
        "SELECT CONCAT_WS('|',slot_id,COALESCE(tenant_id,'NULL'),COALESCE(placement_generation,0),COALESCE(switch_token,'NULL')) AS item FROM biz_tenant_target_slot ORDER BY slot_id",
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
