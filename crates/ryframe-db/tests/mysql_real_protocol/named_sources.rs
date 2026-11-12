use ryframe_config::DbConnection;
use ryframe_db::{ControlDatabaseCluster, DatabaseNodeKind, ReadConsistency, connection};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

use super::{db_error, execute, mysql, require_count, run_mysql_test};

const CROSS_INSTANCE_SOURCE: &str = "cross-instance";

enum ExpectedInstances {
    Same,
    Separate,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_source_transactions_are_independent_and_never_selected_for_reads() {
    run_mysql_test("source-primary", |primary| async move {
        run_mysql_test("source-legacy", |source| async move {
            verify_sources(primary, source, "legacy", ExpectedInstances::Same).await
        })
        .await;
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_source_transactions_are_independent_across_mysql_instances() {
    if !mysql::integration_enabled() {
        eprintln!("未验收 MySQL 跨实例事务：缺少显式 .local-tests/mysql/config 配置目录");
        return;
    }
    let config = mysql::MySqlTestConfiguration::load().unwrap_or_else(|error| panic!("{error}"));
    let database = config.database();
    let Some(source) = database
        .sources
        .iter()
        .find(|source| source.name == CROSS_INSTANCE_SOURCE)
    else {
        eprintln!("未验收 MySQL 跨实例事务：未配置 database.sources 中的 cross-instance");
        return;
    };

    // 两端在任何 fixture 写入前都核对 TLS 配置、真实实例和探测连接关闭。
    let primary = mysql::connection_config(&database.primary, "mysql", None)
        .unwrap_or_else(|error| panic!("primary 配置无效：{error}"));
    let source = mysql::connection_config(&source.connection, "mysql", None)
        .unwrap_or_else(|error| panic!("cross-instance 配置无效：{error}"));
    let primary_uuid = probe_server_uuid(&primary, "primary")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let source_uuid = probe_server_uuid(&source, CROSS_INSTANCE_SOURCE)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_ne!(
        primary_uuid, source_uuid,
        "两侧 MySQL 实例 UUID 相同，尚未建库"
    );

    let scope = config.scope_id();
    mysql::run_mysql_test_with_configuration(
        "source-primary-cross",
        scope,
        &primary,
        Some(&primary_uuid),
        |primary| async move {
            mysql::run_mysql_test_with_configuration(
                "source-legacy-cross",
                scope,
                &source,
                Some(&source_uuid),
                |source| async move {
                    verify_sources(
                        primary,
                        source,
                        CROSS_INSTANCE_SOURCE,
                        ExpectedInstances::Separate,
                    )
                    .await
                },
            )
            .await;
            Ok(())
        },
    )
    .await;
}

async fn probe_server_uuid(config: &DbConnection, name: &str) -> Result<String, String> {
    let database = connection::connect(config)
        .await
        .map_err(|_| format!("{name} 只读预检连接失败，尚未建库"))?;
    let identity = connection::physical_identity(&database)
        .await
        .map_err(|_| format!("{name} 无法确认 MySQL 实例身份，尚未建库"));
    let closed = database
        .close()
        .await
        .map_err(|_| format!("{name} 只读预检连接关闭未确认，尚未建库"));
    closed?;
    Ok(identity?.server_uuid)
}

async fn verify_sources(
    primary: DatabaseConnection,
    source: DatabaseConnection,
    source_name: &str,
    expected_instances: ExpectedInstances,
) -> Result<(), String> {
    let cluster = ControlDatabaseCluster::with_sources_and_replica_slots(
        primary,
        std::iter::empty(),
        [(source_name.to_owned(), source)],
    );
    let source = cluster
        .source(source_name)
        .ok_or("已登记的命名来源不可用")?;
    assert!(cluster.source("unknown-source").is_none());
    let primary_identity = identity(cluster.write()).await?;
    let source_identity = identity(source).await?;
    match expected_instances {
        ExpectedInstances::Same => {
            assert_eq!(primary_identity.0, source_identity.0);
            assert_ne!(primary_identity.2, source_identity.2);
        }
        ExpectedInstances::Separate => assert_ne!(primary_identity.0, source_identity.0),
    }
    assert_ne!(primary_identity.1, source_identity.1);
    for database in [cluster.write(), source] {
        execute(
            database,
            "CREATE TABLE protocol_source_account (id BIGINT NOT NULL PRIMARY KEY) ENGINE=InnoDB",
        )
        .await?;
    }

    verify_independent_transactions(&cluster, source).await?;
    // 两种读取均须实际连接 primary schema 并读取 primary 数据。
    for consistency in [ReadConsistency::Strong, ReadConsistency::Eventual] {
        let selected = cluster.select_read(consistency);
        assert_eq!(selected.kind, DatabaseNodeKind::Primary);
        assert_eq!(selected.node_name.as_ref(), "primary");
        let selected_identity = identity(&selected.connection).await?;
        assert_eq!(selected_identity.0, primary_identity.0);
        assert_eq!(selected_identity.1, primary_identity.1);
        require_count(
            &selected.connection,
            "SELECT COUNT(*) AS value FROM protocol_source_account WHERE id=3",
            1,
        )
        .await?;
        require_count(
            &selected.connection,
            "SELECT COUNT(*) AS value FROM protocol_source_account WHERE id=2",
            0,
        )
        .await?;
    }
    Ok(())
}

async fn verify_independent_transactions(
    cluster: &ControlDatabaseCluster,
    source: &DatabaseConnection,
) -> Result<(), String> {
    // source 已独立提交，随后 primary 回滚不能撤销它。
    let primary_transaction = cluster.write().begin().await.map_err(db_error)?;
    let source_transaction = source.begin().await.map_err(db_error)?;
    execute(
        &primary_transaction,
        "INSERT INTO protocol_source_account (id) VALUES (1)",
    )
    .await?;
    execute(
        &source_transaction,
        "INSERT INTO protocol_source_account (id) VALUES (2)",
    )
    .await?;
    source_transaction.commit().await.map_err(db_error)?;
    primary_transaction.rollback().await.map_err(db_error)?;
    require_count(
        cluster.write(),
        "SELECT COUNT(*) AS value FROM protocol_source_account",
        0,
    )
    .await?;
    require_count(
        source,
        "SELECT COUNT(*) AS value FROM protocol_source_account WHERE id=2",
        1,
    )
    .await?;

    // 对称方向：primary 已提交，source 回滚仅影响自己的写入。
    let primary_transaction = cluster.write().begin().await.map_err(db_error)?;
    let source_transaction = source.begin().await.map_err(db_error)?;
    execute(
        &primary_transaction,
        "INSERT INTO protocol_source_account (id) VALUES (3)",
    )
    .await?;
    execute(
        &source_transaction,
        "INSERT INTO protocol_source_account (id) VALUES (4)",
    )
    .await?;
    primary_transaction.commit().await.map_err(db_error)?;
    source_transaction.rollback().await.map_err(db_error)?;
    require_count(
        cluster.write(),
        "SELECT COUNT(*) AS value FROM protocol_source_account WHERE id=3",
        1,
    )
    .await?;
    require_count(
        source,
        "SELECT COUNT(*) AS value FROM protocol_source_account WHERE id=4",
        0,
    )
    .await?;
    require_count(
        source,
        "SELECT COUNT(*) AS value FROM protocol_source_account",
        1,
    )
    .await?;

    Ok(())
}

async fn identity(database: &DatabaseConnection) -> Result<(String, String, i64), String> {
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        "SELECT @@server_uuid AS server_uuid, DATABASE() AS schema_name, CAST(CONNECTION_ID() AS SIGNED) AS connection_id"))
        .await.map_err(db_error)?.ok_or("MySQL 来源未返回物理身份")?;
    Ok((
        row.try_get("", "server_uuid").map_err(db_error)?,
        row.try_get("", "schema_name").map_err(db_error)?,
        row.try_get("", "connection_id").map_err(db_error)?,
    ))
}
