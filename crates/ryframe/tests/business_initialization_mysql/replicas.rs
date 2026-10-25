use super::*;
use futures_util::FutureExt;
use ryframe_db::{ControlDatabaseCluster, DatabaseNodeHealth, DatabaseNodeKind, ReadConsistency};
use std::{panic::AssertUnwindSafe, time::Duration};
use tokio::task::JoinHandle;

#[path = "replicas/configuration.rs"]
mod configuration;
#[path = "replicas/diagnostics.rs"]
mod diagnostics;
#[path = "replicas/transport.rs"]
mod transport;
use configuration::{Configuration, REPLICA};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_replica_monitor_routes_recovers_and_rejects_business_drift() {
    if !mysql::integration_enabled() {
        eprintln!("副本真实场景未验收：缺少显式 MySQL 文件配置");
        return;
    }
    let binding = Configuration::load().unwrap_or_else(|error| panic!("{error}"));
    ryframe_db::migration::register_business_tables([("biz_sql_example", TABLE_SCHEMA)]).unwrap();
    mysql::run_mysql_test("replica-primary", move |primary| async move {
        mysql::run_mysql_test("replica-read", move |replica| async move {
            scenario(primary, replica, binding).await
        })
        .await;
        Ok(())
    })
    .await;
}

async fn scenario(
    primary: DatabaseConnection,
    replica: DatabaseConnection,
    binding: Configuration,
) -> Result<(), String> {
    binding.verify()?;
    initialize(&primary, "primary-only").await?;
    initialize(&replica, "replica-only").await?;
    let primary_id = identity(&primary).await?;
    let replica_id = identity(&replica).await?;
    assert_eq!(
        primary_id.0, replica_id.0,
        "本例只验收同实例双 schema，不声称 MySQL 复制或双实例"
    );
    assert_ne!(primary_id.1, replica_id.1);
    eprintln!(
        "replica owned identities: server_uuid={} primary={} replica={}",
        primary_id.0, primary_id.1, replica_id.1
    );
    let proxy = transport::Proxy::start(&binding.app.database.primary).await?;
    let config = binding.derive(&primary_id.1, &replica_id.1, proxy.port())?;
    diagnostics::install(&config)?;
    let cluster = ryframe::boot::datasource::connect(&config)
        .await
        .map_err(message)?;
    let mut runtime = Runtime {
        cluster,
        monitor: None,
        proxy,
        pools: Vec::new(),
    };
    let mut result = AssertUnwindSafe(exercise(
        &mut runtime,
        &replica,
        &binding,
        &config,
        &primary_id,
        &replica_id,
    ))
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err("副本真实场景断言 panic；清理测试进程/池后由 owned fixture 保留数据库证据".into())
    });
    if let Err(error) = runtime.close().await {
        result = Err(format!(
            "{}；副本资源收尾失败：{error}",
            result.err().unwrap_or_default()
        ));
    }
    if let Err(error) = binding.verify() {
        result = Err(format!("{}；{error}", result.err().unwrap_or_default()));
    }
    result
}

async fn initialize(database: &DatabaseConnection, code: &str) -> Result<(), String> {
    ryframe_db::migration::up(database, Some(&hashes()?))
        .await
        .map_err(mysql::db_error)?;
    migration(false).up(database).await.map_err(message)?;
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO biz_sql_example(code) VALUES (?)",
            [code.into()],
        ))
        .await
        .map_err(mysql::db_error)?;
    Ok(())
}

async fn exercise(
    runtime: &mut Runtime,
    replica: &DatabaseConnection,
    binding: &Configuration,
    config: &ryframe_config::AppConfig,
    primary_id: &(String, String),
    replica_id: &(String, String),
) -> Result<(), String> {
    let initial = health(&runtime.cluster)?;
    assert!(!initial.healthy && initial.consecutive_successes == 0);
    assert!(runtime.cluster.replica_connection(REPLICA).is_none());
    read(
        &runtime.cluster,
        ReadConsistency::Eventual,
        DatabaseNodeKind::Primary,
        primary_id,
        "primary-only",
    )
    .await?;
    runtime.monitor = Some(ryframe::boot::datasource::spawn_replica_health_monitor(
        runtime.cluster.clone(),
        config.database.replicas.clone(),
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    ));
    admission(runtime, primary_id, replica_id, "initial").await?;
    binding.verify()?;
    disconnect_and_restore(runtime, primary_id, replica_id).await?;
    binding.verify()?;
    business_drift(runtime, replica, primary_id, replica_id).await?;
    read(
        &runtime.cluster,
        ReadConsistency::Strong,
        DatabaseNodeKind::Primary,
        primary_id,
        "primary-only",
    )
    .await?;
    Ok(())
}

async fn admission(
    runtime: &mut Runtime,
    primary: &(String, String),
    replica: &(String, String),
    phase: &str,
) -> Result<(), String> {
    wait_health(&runtime.cluster, phase, |h| {
        !h.healthy && h.consecutive_successes == 1
    })
    .await?;
    read(
        &runtime.cluster,
        ReadConsistency::Eventual,
        DatabaseNodeKind::Primary,
        primary,
        "primary-only",
    )
    .await?;
    wait_health(&runtime.cluster, phase, |h| {
        h.healthy && h.consecutive_successes >= 2
    })
    .await?;
    let connection = runtime
        .cluster
        .replica_connection(REPLICA)
        .ok_or("健康副本缺少真实连接")?;
    runtime.pools.push(connection);
    read(
        &runtime.cluster,
        ReadConsistency::Strong,
        DatabaseNodeKind::Primary,
        primary,
        "primary-only",
    )
    .await?;
    read(
        &runtime.cluster,
        ReadConsistency::Eventual,
        DatabaseNodeKind::Replica,
        replica,
        "replica-only",
    )
    .await
}

async fn disconnect_and_restore(
    runtime: &mut Runtime,
    primary: &(String, String),
    replica: &(String, String),
) -> Result<(), String> {
    fresh_probe(&runtime.cluster).await?;
    let selected = runtime.cluster.select_read(ReadConsistency::Eventual);
    assert_eq!(selected.kind, DatabaseNodeKind::Replica);
    let transaction = selected
        .connection
        .begin_with_config(None, Some(sea_orm::AccessMode::ReadOnly))
        .await
        .map_err(mysql::db_error)?;
    assert_eq!(identity(&transaction).await?, *replica);
    let before = health(&runtime.cluster)?;
    runtime.proxy.disconnect().await?;
    // 使用已选副本的真实只读连接，断链后不让连接池重建掩盖这次 SQL 的失败。
    let failed = transaction
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT code FROM biz_sql_example WHERE id=1",
        ))
        .await;
    assert!(
        failed.is_err(),
        "断链后的已选副本 SQL 必须失败，不能自动回主重放"
    );
    drop(transaction);
    let after = health(&runtime.cluster)?;
    assert_eq!(
        after.consecutive_failures, before.consecutive_failures,
        "请求不得推进阈值；本阶段已对齐刚完成的真实探测周期"
    );
    assert!(after.healthy);
    read(
        &runtime.cluster,
        ReadConsistency::Strong,
        DatabaseNodeKind::Primary,
        primary,
        "primary-only",
    )
    .await?;
    wait_health(&runtime.cluster, "failure-one", |h| {
        h.healthy && h.consecutive_failures == 1
    })
    .await?;
    wait_health(&runtime.cluster, "failure-two", |h| {
        h.healthy && h.consecutive_failures == 2
    })
    .await?;
    wait_health(&runtime.cluster, "removed", |h| {
        !h.healthy && h.consecutive_failures >= 3
    })
    .await?;
    assert!(runtime.cluster.replica_connection(REPLICA).is_none());
    selected.connection.close().await.map_err(mysql::db_error)?;
    read(
        &runtime.cluster,
        ReadConsistency::Eventual,
        DatabaseNodeKind::Primary,
        primary,
        "primary-only",
    )
    .await?;
    runtime.proxy.restore().await?;
    admission(runtime, primary, replica, "reconnected").await
}

async fn business_drift(
    runtime: &mut Runtime,
    replica: &DatabaseConnection,
    primary_id: &(String, String),
    replica_id: &(String, String),
) -> Result<(), String> {
    fresh_probe(&runtime.cluster).await?;
    let before = business_rows(replica).await?;
    mysql::execute(
        replica,
        "ALTER TABLE biz_sql_example MODIFY code VARCHAR(64) NOT NULL",
    )
    .await?;
    let drifted = ryframe_db::schema::canonical_table_schema(replica, "biz_sql_example")
        .await
        .map_err(mysql::db_error)?;
    assert_ne!(drifted, TABLE_SCHEMA);
    let removed = wait_health(&runtime.cluster, "business-schema-rejected", |h| !h.healthy).await?;
    assert_eq!(
        removed.consecutive_failures, 0,
        "业务结构失败必须立即摘除，不经过三次连通失败阈值"
    );
    assert!(runtime.cluster.replica_connection(REPLICA).is_none());
    read(
        &runtime.cluster,
        ReadConsistency::Eventual,
        DatabaseNodeKind::Primary,
        primary_id,
        "primary-only",
    )
    .await?;
    assert_eq!(
        business_rows(replica).await?,
        before,
        "监督器不得修改业务行"
    );
    assert_eq!(
        ryframe_db::schema::canonical_table_schema(replica, "biz_sql_example")
            .await
            .map_err(mysql::db_error)?,
        drifted,
        "监督器不得自动修表"
    );
    // 仅在上述已确认断言后显式恢复这次已成功注入的 DDL；失败不自动重放。
    mysql::execute(
        replica,
        "ALTER TABLE biz_sql_example MODIFY code VARCHAR(32) NOT NULL",
    )
    .await?;
    assert_eq!(
        ryframe_db::schema::canonical_table_schema(replica, "biz_sql_example")
            .await
            .map_err(mysql::db_error)?,
        TABLE_SCHEMA
    );
    admission(runtime, primary_id, replica_id, "schema-restored").await?;
    assert_eq!(business_rows(replica).await?, before);
    Ok(())
}

struct Runtime {
    cluster: ControlDatabaseCluster,
    monitor: Option<JoinHandle<()>>,
    proxy: transport::Proxy,
    pools: Vec<DatabaseConnection>,
}

impl Runtime {
    async fn close(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(task) = self.monitor.take() {
            task.abort();
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                errors.push("副本监督器异常退出".to_owned());
            }
        }
        if let Some(connection) = self.cluster.replica_connection(REPLICA) {
            self.pools.push(connection);
        }
        if let Err(error) = self.proxy.close().await {
            errors.push(error);
        }
        for connection in self.pools.drain(..) {
            if let Err(error) = connection.close().await {
                errors.push(mysql::db_error(error));
            }
        }
        if let Err(error) = self.cluster.write().clone().close().await {
            errors.push(mysql::db_error(error));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(task) = self.monitor.take() {
            task.abort();
        }
        // 正常和捕获的失败均显式 close；取消时 Proxy 的 Drop 中止连接转发。
    }
}

fn health(cluster: &ControlDatabaseCluster) -> Result<DatabaseNodeHealth, String> {
    cluster
        .replica_health()
        .into_iter()
        .find(|h| h.name == REPLICA)
        .ok_or_else(|| "副本健康槽丢失".into())
}

async fn fresh_probe(cluster: &ControlDatabaseCluster) -> Result<(), String> {
    let previous = health(cluster)?.consecutive_successes;
    wait_health(cluster, "fresh-success", |h| {
        h.healthy && h.consecutive_successes > previous
    })
    .await
    .map(|_| ())
}

async fn wait_health(
    cluster: &ControlDatabaseCluster,
    stage: &str,
    ready: impl Fn(&DatabaseNodeHealth) -> bool,
) -> Result<DatabaseNodeHealth, String> {
    // 真实 5 秒周期和最长 60 秒重连退避；只轮询观察，不推进生产计数。
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let state = health(cluster)?;
            if ready(&state) {
                eprintln!(
                    "replica {stage}: healthy={} failures={} successes={}",
                    state.healthy, state.consecutive_failures, state.consecutive_successes
                );
                return Ok(state);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| {
        format!(
            "副本阶段 {stage} 超时，最后健康状态：{:?}",
            cluster.replica_health()
        )
    })?
}

async fn identity<C: ConnectionTrait>(database: &C) -> Result<(String, String), String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@server_uuid AS server_uuid, DATABASE() AS schema_name",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("MySQL 未返回实际实例/schema 身份")?;
    Ok((
        row.try_get("", "server_uuid").map_err(mysql::db_error)?,
        row.try_get("", "schema_name").map_err(mysql::db_error)?,
    ))
}

async fn read(
    cluster: &ControlDatabaseCluster,
    consistency: ReadConsistency,
    kind: DatabaseNodeKind,
    expected: &(String, String),
    code: &str,
) -> Result<(), String> {
    let selected = cluster.select_read(consistency);
    assert_eq!(selected.kind, kind);
    let transaction = selected
        .connection
        .begin_with_config(None, Some(sea_orm::AccessMode::ReadOnly))
        .await
        .map_err(mysql::db_error)?;
    assert_eq!(identity(&transaction).await?, *expected);
    let row = transaction
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT code FROM biz_sql_example WHERE id=1",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("缺少实际读取的业务行")?;
    assert_eq!(
        row.try_get::<String>("", "code").map_err(mysql::db_error)?,
        code
    );
    let cipher = transaction
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SHOW SESSION STATUS LIKE 'Ssl_cipher'",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("缺少同一连接的 TLS 状态")?;
    assert!(
        !cipher
            .try_get::<String>("", "Value")
            .map_err(mysql::db_error)?
            .is_empty(),
        "选定连接必须实际协商 TLS"
    );
    transaction.rollback().await.map_err(mysql::db_error)
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
