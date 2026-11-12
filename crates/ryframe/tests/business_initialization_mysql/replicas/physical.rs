use super::*;
use ryframe_config::{DbConnection, DbTlsMode};

const CODE: &str = "copied-from-source";
const EMPTY: &str = "SELECT COUNT(*) AS value FROM information_schema.tables \
                     WHERE table_schema = DATABASE()";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_replica_monitor_routes_across_mysql_instances() {
    if !mysql::integration_enabled() {
        eprintln!("物理副本场景未验收：缺少显式 MySQL 文件配置");
        return;
    }
    let binding = Configuration::load().unwrap_or_else(|error| panic!("{error}"));
    let Some(replica) = binding.physical() else {
        eprintln!("物理副本场景未验收：未登记 physical-replica");
        return;
    };
    scenario(&binding, replica)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

async fn scenario(binding: &Configuration, replica: &DbConnection) -> Result<(), String> {
    let primary = &binding.app.database.primary;
    let schema = format!(
        "ryframe_replica_{}",
        binding.app.scope_id.as_str().replace('-', "_")
    );
    if primary.database != schema
        || replica.database != schema
        || primary.port == replica.port
        || !matches!(replica.host.as_str(), "localhost" | "127.0.0.1" | "::1")
        || replica.tls_mode == DbTlsMode::Disabled
    {
        return Err("物理副本仅接受异端口回环 TLS 连接和由 scope_id 绑定的同名隔离库".into());
    }
    let proxy = transport::Proxy::start(replica).await?;
    if proxy.port() == primary.port {
        return Err("物理副本代理与主库端口重合".into());
    }
    let config = binding.with_proxy(&schema, replica.clone(), proxy.port())?;
    let mut runtime = Runtime::start(&config, proxy).await?;
    let result = AssertUnwindSafe(exercise_physical(&mut runtime, binding, &config, replica))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err("物理副本真实场景断言 panic，保留隔离库现场".into()));
    let closed = runtime.close().await;
    let verified = binding.verify();
    result.and(closed).and(verified)
}

async fn exercise_physical(
    runtime: &mut Runtime,
    binding: &Configuration,
    config: &ryframe_config::AppConfig,
    replica_config: &DbConnection,
) -> Result<(), String> {
    let replica = ryframe_db::connection::connect(replica_config)
        .await
        .map_err(|_| "无法连接已登记的物理副本")?;
    runtime.pools.push(replica.clone());
    let source = runtime.cluster.write();
    let source_id = identity(source).await?;
    let replica_id = identity(&replica).await?;
    if source_id.0 == replica_id.0
        || source_id.1 != binding.app.database.primary.database
        || replica_id.1 != source_id.1
    {
        return Err("物理副本实例或隔离库身份不符，未初始化".into());
    }
    mysql::require_count(
        source,
        "SELECT COUNT(*) AS value FROM DUAL WHERE @@global.log_bin = 1 AND @@global.read_only = 0",
        1,
    )
    .await?;
    verify_channel(&replica, &source_id.0).await?;
    mysql::require_count(source, EMPTY, 0).await?;
    mysql::require_count(&replica, EMPTY, 0).await?;
    binding.verify()?;
    ryframe_db::migration::register_business_tables([("biz_sql_example", TABLE_SCHEMA)])
        .map_err(mysql::db_error)?;
    initialize(source, CODE).await?;
    wait_for_row(&replica).await?;
    verify_channel(&replica, &source_id.0).await?;
    eprintln!(
        "replica physical copy: source_uuid={} replica_uuid={} schema={} row={CODE}",
        source_id.0, replica_id.0, source_id.1
    );
    let codes = [CODE; 2];
    runtime.start_monitor(config, &source_id, CODE).await?;
    admission(runtime, &source_id, &replica_id, "physical-initial", codes).await?;
    disconnect_and_restore(runtime, &source_id, &replica_id, codes).await?;
    verify_channel(&replica, &source_id.0).await
}

async fn verify_channel(replica: &DatabaseConnection, source: &str) -> Result<(), String> {
    let row = replica
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT SOURCE_UUID AS source_uuid FROM performance_schema.replication_connection_status \
             WHERE CHANNEL_NAME = 'ryframe_fixture' AND SERVICE_STATE = 'ON' \
             AND LAST_ERROR_NUMBER = 0 AND @@global.super_read_only = 1 \
             AND EXISTS (SELECT 1 FROM performance_schema.replication_applier_status \
                         WHERE CHANNEL_NAME = 'ryframe_fixture' AND SERVICE_STATE = 'ON')",
        ))
        .await
        .map_err(mysql::db_error)?
        .ok_or("物理副本未运行已登记的只读复制通道")?;
    let actual: String = row.try_get("", "source_uuid").map_err(mysql::db_error)?;
    if !actual.eq_ignore_ascii_case(source) {
        return Err("复制通道指向其他 MySQL 实例".into());
    }
    Ok(())
}

async fn wait_for_row(replica: &DatabaseConnection) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let table = replica
                .query_one_raw(Statement::from_string(
                    DbBackend::MySql,
                    "SELECT 1 FROM information_schema.tables WHERE table_schema = DATABASE() \
                     AND table_name = 'biz_sql_example'",
                ))
                .await
                .map_err(mysql::db_error)?;
            if table.is_some()
                && let Some(row) = replica
                    .query_one_raw(Statement::from_string(
                        DbBackend::MySql,
                        "SELECT code FROM biz_sql_example WHERE id=1",
                    ))
                    .await
                    .map_err(mysql::db_error)?
            {
                let actual: String = row.try_get("", "code").map_err(mysql::db_error)?;
                return if actual == CODE {
                    Ok(())
                } else {
                    Err("副本收到意外业务值，保留现场".into())
                };
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "源已写入，副本未在 60 秒内观察到标记，保留现场")?
}
