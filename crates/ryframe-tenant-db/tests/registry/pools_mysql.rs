//! 复用现有 owned MySQL 夹具，只验收注册表真实池生命周期，不替代租户结构或路由验收。
use super::mysql;
use futures_util::FutureExt;
use ryframe_config::{
    DbConnection, SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig,
    TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use ryframe_tenant_db::{TenantDataError, TenantDatabasePoolLease, TenantDatabaseTargetRegistry};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use std::{panic::AssertUnwindSafe, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_budget_concurrency_and_idle_rebuild_use_real_mysql() {
    mysql::run_mysql_test("registry-first", |first| async move {
        mysql::run_mysql_test("registry-second", |second| async move {
            exercise(&first, &second).await
        })
        .await;
        Ok(())
    })
    .await;
}

async fn exercise(first: &DatabaseConnection, second: &DatabaseConnection) -> Result<(), String> {
    let first_schema = schema(first).await?;
    let second_schema = schema(second).await?;
    assert_ne!(first_schema, second_schema);
    let configuration = mysql::MySqlTestConfiguration::load()?;
    let targets = vec![
        target("first", configuration.connection(&first_schema, None)?, 1),
        target("second", configuration.connection(&second_schema, None)?, 2),
    ];
    for database in [first, second] {
        mysql::execute(
            database,
            "CREATE TABLE registry_pool_probe (id BIGINT NOT NULL PRIMARY KEY)",
        )
        .await?;
        mysql::execute(database, "INSERT INTO registry_pool_probe VALUES (7)").await?;
    }
    let mut pools = Vec::new();
    let result = AssertUnwindSafe(async {
        budget_and_idle(&targets, &first_schema, &second_schema, &mut pools).await?;
        capacity_and_lru(&targets, &first_schema, &second_schema, &mut pools).await
    })
    .catch_unwind()
    .await;
    // 旧池和重建池都登记后显式关闭；发生断言失败也不遗留后台连接干扰 owned 清理。
    let mut close_error = None;
    for pool in pools {
        if pool.close().await.is_err() {
            close_error = Some("registry 真实场景连接池关闭未确认；保留数据库现场".to_owned());
        }
    }
    let result = result.unwrap_or_else(|_| {
        Err("registry 真实行为断言失败；已收尾池，保留 owned 数据库证据".into())
    });
    match (result, close_error) {
        (Err(error), Some(close)) => Err(format!("{error}；{close}")),
        (Err(error), None) => Err(error),
        (Ok(()), Some(error)) => Err(error),
        (Ok(()), None) => Ok(()),
    }
}

async fn budget_and_idle(
    targets: &[TenantDatabaseTargetConfig],
    first_schema: &str,
    second_schema: &str,
    pools: &mut Vec<DatabaseConnection>,
) -> Result<(), String> {
    eprintln!("registry 阶段：连接预算、并发首建及真实后台闲置回收");
    let registry = registry(targets, 2, 2, 1)?;
    let first = concurrent_first(&registry, first_schema, pools).await?;
    let original = connection_id(first.connection()).await?;
    let transaction = first.connection().begin().await.map_err(mysql::db_error)?;
    let transaction_id = connection_id(&transaction).await?;
    // 超过真实 idle 阈值，后台 sweeper 不得回收仍有活动租约及实际事务的池。
    tokio::time::sleep(Duration::from_millis(2200)).await;
    stats(&registry, (1, 0, 1, 1)).await;
    assert_eq!(connection_id(&transaction).await?, transaction_id);
    require_value(&transaction, first_schema).await?;
    transaction.rollback().await.map_err(mysql::db_error)?;
    eprintln!("registry 活动租约跨过 idle 阈值且原事务可读；释放后只观察后台回收");
    let old = first.connection().clone();
    drop(first);
    await_idle_retirement(&registry, &old).await?;
    let renewed = registry.acquire("first").await.map_err(message)?;
    pools.push(renewed.connection().clone());
    let rebuilt = connection_id(renewed.connection()).await?;
    assert_ne!(rebuilt, original, "闲置回收后必须建立新实际连接");
    require_value(renewed.connection(), first_schema).await?;
    let previous = renewed.connection().clone();
    drop(renewed);
    let second = registry.acquire("second").await.map_err(message)?;
    pools.push(second.connection().clone());
    await_closed(&previous).await?;
    require_value(second.connection(), second_schema).await?;
    let one = second.connection().begin().await.map_err(mysql::db_error)?;
    let two = second.connection().begin().await.map_err(mysql::db_error)?;
    assert_ne!(connection_id(&one).await?, connection_id(&two).await?);
    stats(&registry, (1, 0, 1, 2)).await;
    one.rollback().await.map_err(mysql::db_error)?;
    two.rollback().await.map_err(mysql::db_error)?;
    eprintln!(
        "registry budget/concurrency/idle: schema={first_schema} original={original} rebuilt={rebuilt}; second={second_schema}"
    );
    Ok(())
}

async fn concurrent_first(
    registry: &TenantDatabaseTargetRegistry,
    first_schema: &str,
    pools: &mut Vec<DatabaseConnection>,
) -> Result<TenantDatabasePoolLease, String> {
    let results = tokio::time::timeout(
        Duration::from_secs(20),
        futures_util::future::join_all((0..8).map(|_| registry.acquire("first"))),
    )
    .await
    .map_err(|_| "并发首建未在期限内完成")?;
    let mut leases = Vec::new();
    for result in results {
        let lease = result.map_err(message)?;
        pools.push(lease.connection().clone());
        leases.push(lease);
    }
    eprintln!("registry 八个并发 acquire 均已返回；核对单池实际连接及预算拒绝");
    let first = leases.pop().ok_or("并发获取未返回租约")?;
    let original = connection_id(first.connection()).await?;
    for lease in &leases {
        assert_eq!(
            connection_id(lease.connection()).await?,
            original,
            "每池最大连接数为 1；并发首建不得创建多个实际池"
        );
        require_value(lease.connection(), first_schema).await?;
    }
    stats(registry, (1, 0, 8, 1)).await;
    assert!(matches!(
        registry.acquire("second").await,
        Err(TenantDataError::ConnectionBudgetExhausted {
            used: 1,
            requested: 2,
            limit: 2
        })
    ));
    stats(registry, (1, 0, 8, 1)).await;
    drop(leases);
    Ok(first)
}

async fn capacity_and_lru(
    targets: &[TenantDatabaseTargetConfig],
    first_schema: &str,
    second_schema: &str,
    pools: &mut Vec<DatabaseConnection>,
) -> Result<(), String> {
    eprintln!("registry 阶段：目标数量预算、活动租约保护及压力 LRU 重建");
    let registry = registry(targets, 1, 3, 60)?;
    let first = registry.acquire("first").await.map_err(message)?;
    pools.push(first.connection().clone());
    let original = connection_id(first.connection()).await?;
    assert!(matches!(
        registry.acquire("second").await,
        Err(TenantDataError::PoolCapacityExhausted {
            open_targets: 1,
            max_open_targets: 1
        })
    ));
    stats(&registry, (1, 0, 1, 1)).await;
    assert_eq!(connection_id(first.connection()).await?, original);
    require_value(first.connection(), first_schema).await?;
    let old = first.connection().clone();
    drop(first);
    // 不等待 60 秒 TTL，真实容量压力只能回收无活动租约的旧池。
    let second = registry.acquire("second").await.map_err(message)?;
    pools.push(second.connection().clone());
    await_closed(&old).await?;
    require_value(second.connection(), second_schema).await?;
    stats(&registry, (1, 0, 1, 2)).await;
    let old = second.connection().clone();
    drop(second);
    let renewed = registry.acquire("first").await.map_err(message)?;
    pools.push(renewed.connection().clone());
    await_closed(&old).await?;
    assert_ne!(connection_id(renewed.connection()).await?, original);
    require_value(renewed.connection(), first_schema).await?;
    stats(&registry, (1, 0, 1, 1)).await;
    eprintln!(
        "registry capacity/LRU: active lease protected; first/second/first physical reads confirmed"
    );
    Ok(())
}

fn registry(
    targets: &[TenantDatabaseTargetConfig],
    max_open_targets: usize,
    max_total_connections: u32,
    idle_pool_secs: u64,
) -> Result<TenantDatabaseTargetRegistry, String> {
    TenantDatabaseTargetRegistry::new(
        &TenantDataConfig {
            targets: targets.to_vec(),
            max_open_targets,
            max_total_connections,
            idle_pool_secs,
            ..TenantDataConfig::default()
        },
        SqlLogLevel::Off,
        200,
    )
    .map_err(message)
}

fn target(key: &str, connection: DbConnection, maximum: u32) -> TenantDatabaseTargetConfig {
    TenantDatabaseTargetConfig {
        key: key.into(),
        kind: TenantDatabaseTargetKind::Mysql,
        mode: TenantDatabaseTargetMode::Shared,
        host: Some(connection.host),
        port: Some(connection.port),
        database: Some(connection.database),
        username: Some(connection.username),
        password: Some(connection.password),
        tls_mode: Some(connection.tls_mode),
        tls_ca: connection.tls_ca,
        tls_client_cert: connection.tls_client_cert,
        tls_client_key: connection.tls_client_key,
        max_connections: Some(maximum),
        ..TenantDatabaseTargetConfig::shared_control()
    }
}

async fn stats(registry: &TenantDatabaseTargetRegistry, expected: (usize, usize, usize, u32)) {
    let stats = registry.pool_stats().await;
    assert_eq!(
        (
            stats.open_targets,
            stats.opening_targets,
            stats.active_leases,
            stats.reserved_connections
        ),
        expected
    );
}

async fn await_idle_retirement(
    registry: &TenantDatabaseTargetRegistry,
    pool: &DatabaseConnection,
) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = registry.pool_stats().await;
            if state.open_targets == 0
                && state.opening_targets == 0
                && state.reserved_connections == 0
                && pool.get_mysql_connection_pool().is_closed()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "真实后台 sweeper 未完成闲置池移除及关闭；不手工推进回收".into())
}

async fn await_closed(pool: &DatabaseConnection) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !pool.get_mysql_connection_pool().is_closed() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "LRU 淘汰的实际连接池未关闭".into())
}

async fn connection_id<C: ConnectionTrait>(database: &C) -> Result<i64, String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT CAST(CONNECTION_ID() AS SIGNED) AS value",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("MySQL 未返回实际连接身份")?;
    row.try_get("", "value").map_err(mysql::db_error)
}

async fn schema(database: &DatabaseConnection) -> Result<String, String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE() AS value",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("MySQL 未返回明确 schema")?;
    row.try_get("", "value").map_err(mysql::db_error)
}

async fn require_value<C: ConnectionTrait>(database: &C, expected: &str) -> Result<(), String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DATABASE() AS schema_name, id FROM registry_pool_probe",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("实际目标库缺少探针行")?;
    assert_eq!(
        row.try_get::<String>("", "schema_name")
            .map_err(mysql::db_error)?,
        expected
    );
    assert_eq!(row.try_get::<i64>("", "id").map_err(mysql::db_error)?, 7);
    Ok(())
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
