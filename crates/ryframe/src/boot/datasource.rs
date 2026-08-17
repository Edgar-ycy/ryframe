use ryframe_config::{AppConfig, DatabaseReplicaConfig, SqlLogLevel};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use sea_orm::DatabaseConnection;
use std::time::{Duration, Instant};

const REPLICA_HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const REPLICA_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const REPLICA_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(60);

/// 连接主数据库、可选副本和具名数据源。
/// 主库和具名数据源连接失败仍会导致启动失败；副本以降级状态启动，避免短暂的副本
/// 故障阻止 API 提供由主库支撑的服务。
pub async fn connect(config: &AppConfig) -> Result<ControlDatabaseCluster, AppError> {
    let primary_config = &config.database.primary;
    let primary = ryframe_db::connection::connect_with_sql_logging(
        primary_config,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await
    .map_err(|error| AppError::Database(format!("primary database connection failed: {error}")))?;
    ryframe_db::connection::ping(&primary)
        .await
        .map_err(|error| {
            AppError::Database(format!("primary database health check failed: {error}"))
        })?;
    tracing::info!(
        database = %primary_config.database,
        driver = "mysql",
        "primary database connected"
    );

    // 副本始终以空槽位注册。启动不等待副本，迁移完成后由监督器在受限超时内连接、
    // PING 并校验结构；连续两次完整成功前不会参与读路由。
    let configured_replicas = &config.database.replicas;
    let replicas = configured_replicas
        .iter()
        .map(|replica| (replica.name.clone(), None, false));

    let mut sources = Vec::with_capacity(config.database.sources.len());
    for source_config in &config.database.sources {
        let source = ryframe_db::connection::connect_with_sql_logging(
            &source_config.connection,
            config.database.sql_log_level,
            config.database.sql_slow_threshold_ms,
        )
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "named database source {} connection failed: {error}",
                source_config.name
            ))
        })?;
        ryframe_db::connection::ping(&source)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "named database source {} health check failed: {error}",
                    source_config.name
                ))
            })?;
        tracing::info!(
            source = %source_config.name,
            database = %source_config.connection.database,
            driver = "mysql",
            "named database source connected"
        );
        sources.push((source_config.name.clone(), source));
    }

    Ok(ControlDatabaseCluster::with_sources_and_replica_slots(
        primary, replicas, sources,
    ))
}

/// 校验主库迁移账本与结构。滚动迁移期间副本可能滞后，健康检查通过前不得参与路由。
pub async fn verify_schema(cluster: &ControlDatabaseCluster) -> Result<(), AppError> {
    verify_database_node("primary", cluster.write()).await?;
    tracing::info!(
        node = "primary",
        "database migration ledger and schema fingerprint verified"
    );
    Ok(())
}

/// 持续探测副本，但不将其纳入就绪判定。返回的句柄由应用进程持有，并在优雅退出时
/// 随 Tokio 运行时一同释放。
pub fn spawn_replica_health_monitor(
    database: ControlDatabaseCluster,
    replicas: Vec<DatabaseReplicaConfig>,
    sql_log_level: SqlLogLevel,
    sql_slow_threshold_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut states = replicas
            .iter()
            .map(|_| ReplicaReconnectState::new())
            .collect::<Vec<_>>();
        let mut interval = tokio::time::interval(REPLICA_HEALTH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            for (replica, state) in replicas.iter().zip(states.iter_mut()) {
                probe_replica(
                    &database,
                    replica,
                    sql_log_level,
                    sql_slow_threshold_ms,
                    state,
                )
                .await;
            }
        }
    })
}

/// 单个副本的重连退避状态；后台监督器是唯一会推进它的写入方。
#[derive(Debug, Clone)]
struct ReplicaReconnectState {
    next_attempt_at: Instant,
    next_delay: Duration,
    healthy: Option<bool>,
}

impl ReplicaReconnectState {
    fn new() -> Self {
        Self {
            next_attempt_at: Instant::now(),
            next_delay: REPLICA_HEALTH_INTERVAL,
            healthy: None,
        }
    }

    fn is_due(&self, now: Instant) -> bool {
        now >= self.next_attempt_at
    }

    fn record_success(&mut self) -> bool {
        let changed = self.healthy != Some(true);
        self.healthy = Some(true);
        self.next_attempt_at = Instant::now();
        self.next_delay = REPLICA_HEALTH_INTERVAL;
        changed
    }

    fn record_failure(&mut self) -> bool {
        let changed = self.healthy != Some(false);
        self.healthy = Some(false);
        let delay = self.next_delay;
        self.next_attempt_at = Instant::now() + delay;
        self.next_delay = delay.saturating_mul(2).min(REPLICA_RECONNECT_MAX_DELAY);
        changed
    }

    fn observe_failure(&mut self) -> bool {
        let changed = self.healthy != Some(false);
        self.healthy = Some(false);
        changed
    }
}

enum ReplicaProbeFailure {
    Connectivity(AppError),
    Schema(AppError),
    Timeout,
}

async fn probe_replica(
    database: &ControlDatabaseCluster,
    replica: &DatabaseReplicaConfig,
    sql_log_level: SqlLogLevel,
    sql_slow_threshold_ms: u64,
    state: &mut ReplicaReconnectState,
) {
    if !state.is_due(Instant::now()) {
        return;
    }

    let result = match database.replica_connection(&replica.name) {
        Some(connection) => tokio::time::timeout(
            REPLICA_PROBE_TIMEOUT,
            validate_replica(&replica.name, &connection),
        )
        .await
        .map_err(|_| ReplicaProbeFailure::Timeout)
        .and_then(|result| result)
        .map(|()| None),
        None => tokio::time::timeout(
            REPLICA_PROBE_TIMEOUT,
            connect_and_validate_replica(replica, sql_log_level, sql_slow_threshold_ms),
        )
        .await
        .map_err(|_| ReplicaProbeFailure::Timeout)
        .and_then(|result| result)
        .map(Some),
    };

    match result {
        Ok(Some(connection)) => {
            database.replace_replica_connection(&replica.name, connection);
            database.record_replica_probe(&replica.name, true);
            if state.record_success() {
                tracing::info!(replica = %replica.name, "副本连接和结构校验成功");
            }
        }
        Ok(None) => {
            database.record_replica_probe(&replica.name, true);
            if state.record_success() {
                tracing::info!(replica = %replica.name, "副本健康状态已恢复");
            }
        }
        Err(ReplicaProbeFailure::Schema(error)) => {
            // 结构不一致是安全硬失败：立即摘除连接，后续重连也必须重新校验。
            database.clear_replica_connection(&replica.name);
            if state.record_failure() {
                tracing::warn!(replica = %replica.name, %error, "副本结构校验失败，已从读路由摘除");
            } else {
                tracing::debug!(replica = %replica.name, %error, "副本结构校验仍未恢复");
            }
        }
        Err(error) => {
            database.record_replica_probe(&replica.name, false);
            let changed = state.observe_failure();
            if !database.replica_is_routable(&replica.name).unwrap_or(false) {
                database.clear_replica_connection(&replica.name);
                state.record_failure();
            }
            match error {
                ReplicaProbeFailure::Connectivity(error) if changed => {
                    tracing::warn!(replica = %replica.name, %error, "副本连接或健康检查失败");
                }
                ReplicaProbeFailure::Connectivity(error) => {
                    tracing::debug!(replica = %replica.name, %error, "副本连接或健康检查仍未恢复");
                }
                ReplicaProbeFailure::Timeout if changed => {
                    tracing::warn!(
                        replica = %replica.name,
                        timeout_secs = REPLICA_PROBE_TIMEOUT.as_secs(),
                        "副本探测超时"
                    );
                }
                ReplicaProbeFailure::Timeout => {
                    tracing::debug!(
                        replica = %replica.name,
                        timeout_secs = REPLICA_PROBE_TIMEOUT.as_secs(),
                        "副本探测仍超时"
                    );
                }
                ReplicaProbeFailure::Schema(_) => unreachable!("结构失败已单独处理"),
            }
        }
    }
}

async fn connect_and_validate_replica(
    replica: &DatabaseReplicaConfig,
    sql_log_level: SqlLogLevel,
    sql_slow_threshold_ms: u64,
) -> Result<DatabaseConnection, ReplicaProbeFailure> {
    let connection = ryframe_db::connection::connect_with_sql_logging(
        &replica.connection,
        sql_log_level,
        sql_slow_threshold_ms,
    )
    .await
    .map_err(ReplicaProbeFailure::Connectivity)?;
    validate_replica(&replica.name, &connection).await?;
    Ok(connection)
}

async fn validate_replica(
    name: &str,
    connection: &DatabaseConnection,
) -> Result<(), ReplicaProbeFailure> {
    ryframe_db::connection::ping(connection)
        .await
        .map_err(ReplicaProbeFailure::Connectivity)?;
    verify_database_node(name, connection)
        .await
        .map_err(ReplicaProbeFailure::Schema)
}

/// 按当前迁移账本校验列、索引和外键，账本落后时同样拒绝路由。
async fn verify_database_node(node: &str, db: &DatabaseConnection) -> Result<(), AppError> {
    ryframe_db_migration::verify(db).await.map_err(|error| {
        AppError::Internal(format!(
            "database node {node} migration and schema verification failed: {error}"
        ))
    })?;
    tracing::debug!(
        node,
        "database migration ledger and schema fingerprint verified"
    );
    Ok(())
}
