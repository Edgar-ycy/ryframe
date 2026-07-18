#[path = "common/test_database.rs"]
mod test_database;

use ryframe_db::DatabaseCluster;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TryGetable};
use test_database::{
    TestDatabase, mysql_test_admin_url, validate_test_database_name, validate_test_database_purpose,
};

#[tokio::test]
async fn mysql_named_source_is_distinct_and_explicit() {
    let primary_fixture = TestDatabase::create("named_primary").await;
    let device_fixture = TestDatabase::create("named_device").await;

    device_fixture
        .execute_unprepared(
            "CREATE TABLE `t_device_smoke` (\
                `id` BIGINT NOT NULL AUTO_INCREMENT,\
                `name` VARCHAR(64) NOT NULL,\
                `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,\
                PRIMARY KEY (`id`)\
             ) COMMENT='RyFrame named data source smoke table'",
        )
        .await
        .expect("create device smoke table");

    let cluster = DatabaseCluster::with_sources(
        primary_fixture.connection().clone(),
        std::iter::empty(),
        [(
            "ryframe_device".to_owned(),
            device_fixture.connection().clone(),
        )],
    );

    let primary_name = current_database(cluster.read()).await;
    let source_name = current_database(cluster.source("ryframe_device").unwrap()).await;
    assert_eq!(primary_name, primary_fixture.database_name());
    assert_eq!(source_name, device_fixture.database_name());
    assert_ne!(primary_name, source_name);
    assert!(cluster.source("missing").is_none());
}

#[tokio::test]
async fn test_database_fixture_cleans_up_after_normal_return_and_panic() {
    let normal_name;
    {
        let fixture = TestDatabase::create("normal_cleanup").await;
        normal_name = fixture.database_name().to_owned();
        assert!(database_exists(&normal_name).await);
    }
    assert!(
        !database_exists(&normal_name).await,
        "normal fixture drop must remove the isolated database"
    );

    let join_error = tokio::spawn(async {
        let fixture = TestDatabase::create("panic_cleanup").await;
        std::panic::panic_any(fixture.database_name().to_owned());
    })
    .await
    .expect_err("fixture probe task must panic intentionally");
    let panic_name = *join_error
        .into_panic()
        .downcast::<String>()
        .expect("panic payload contains the isolated database name");
    assert!(
        !database_exists(&panic_name).await,
        "fixture drop during panic unwinding must remove the isolated database"
    );
}

#[test]
fn unsafe_database_identifiers_are_rejected_before_sql_execution() {
    assert!(validate_test_database_purpose("named_source").is_ok());
    assert!(validate_test_database_purpose("../mysql").is_err());
    assert!(validate_test_database_name("ryframe_device").is_err());
    assert!(validate_test_database_name("ryframe_it_safe;drop").is_err());
}

async fn database_exists(database_name: &str) -> bool {
    validate_test_database_name(database_name).expect("database lookup is limited to test names");
    let admin = Database::connect(mysql_test_admin_url())
        .await
        .expect("connect MySQL test admin database");
    let row = admin
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            format!(
                "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SCHEMATA \
                 WHERE SCHEMA_NAME = '{database_name}'"
            ),
        ))
        .await
        .expect("query isolated database existence")
        .expect("database existence count row");
    let count = i64::try_get_by_index(&row, 0).expect("database existence count");
    let _ = admin.close().await;
    count == 1
}

async fn current_database(database: &sea_orm::DatabaseConnection) -> String {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE()".to_owned(),
        ))
        .await
        .expect("query current database")
        .expect("current database row");
    String::try_get_by_index(&row, 0).expect("current database name")
}
