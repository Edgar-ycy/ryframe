use ryframe_db::DatabaseCluster;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TryGetable};

const PRIMARY_URL: &str =
    "mysql://root:123456@127.0.0.1:3306/ryframe_config?collation=utf8mb4_general_ci";
const DEVICE_URL: &str =
    "mysql://root:123456@127.0.0.1:3306/ryframe_device?collation=utf8mb4_general_ci";

#[tokio::test]
#[ignore = "requires a local MySQL instance"]
async fn mysql_named_source_is_distinct_and_explicit() {
    let primary_url =
        std::env::var("RYFRAME_PRIMARY_DATABASE_URL").unwrap_or_else(|_| PRIMARY_URL.to_owned());
    let device_url =
        std::env::var("RYFRAME_DEVICE_DATABASE_URL").unwrap_or_else(|_| DEVICE_URL.to_owned());

    let primary = Database::connect(primary_url)
        .await
        .expect("connect primary");
    primary
        .execute_unprepared(
            "CREATE DATABASE IF NOT EXISTS `ryframe_device` \
             CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci",
        )
        .await
        .expect("create ryframe_device");

    let device = Database::connect(device_url)
        .await
        .expect("connect ryframe_device");
    device
        .execute_unprepared(
            "CREATE TABLE IF NOT EXISTS `t_device_smoke` (\
                `id` BIGINT NOT NULL AUTO_INCREMENT,\
                `name` VARCHAR(64) NOT NULL,\
                `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,\
                PRIMARY KEY (`id`)\
             ) COMMENT='RyFrame named data source smoke table'",
        )
        .await
        .expect("create device smoke table");

    let cluster = DatabaseCluster::with_sources(
        primary,
        std::iter::empty(),
        [("ryframe_device".to_owned(), device)],
    );

    assert_eq!(current_database(cluster.read()).await, "ryframe_config");
    assert_eq!(
        current_database(cluster.source("ryframe_device").unwrap()).await,
        "ryframe_device"
    );
    assert!(cluster.source("missing").is_none());
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
