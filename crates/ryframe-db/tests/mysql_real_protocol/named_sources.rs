use ryframe_db::{ControlDatabaseCluster, DatabaseNodeKind, ReadConsistency};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

use super::{db_error, execute, require_count, run_mysql_test};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_source_transactions_are_independent_and_never_selected_for_reads() {
    run_mysql_test("source-primary", |primary| async move {
        run_mysql_test("source-legacy", |source| async move {
            verify_sources(primary, source).await
        })
        .await;
        Ok(())
    })
    .await;
}

async fn verify_sources(
    primary: DatabaseConnection,
    source: DatabaseConnection,
) -> Result<(), String> {
    let cluster = ControlDatabaseCluster::with_sources_and_replica_slots(
        primary,
        std::iter::empty(),
        [("legacy".to_owned(), source)],
    );
    let source = cluster
        .source("legacy")
        .ok_or("已登记的 legacy 来源不可用")?;
    assert!(cluster.source("unknown-source").is_none());
    let primary_identity = identity(cluster.write()).await?;
    let source_identity = identity(source).await?;
    assert_eq!(
        primary_identity.0, source_identity.0,
        "本例只验证同实例独立 schema，不验证跨实例事务"
    );
    assert_ne!(primary_identity.1, source_identity.1);
    assert_ne!(primary_identity.2, source_identity.2);
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
        assert_eq!(identity(&selected.connection).await?.1, primary_identity.1);
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
