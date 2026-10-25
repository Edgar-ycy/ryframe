use ryframe_config::{AppConfig, DatabaseReplicaConfig, DbConnection, SqlLogLevel};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppError;
use sea_orm::DatabaseConnection;
use std::time::{Duration, Instant};

const REPLICA_HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const REPLICA_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const REPLICA_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(60);

#[cfg(test)]
#[path = "../../tests/boot_contract/datasource_diagnostics.rs"]
mod tests;

/// 连接主数据库、可选副本和具名数据源。
/// 主库和具名数据源连接失败仍会导致启动失败；副本以降级状态启动，避免短暂的副本
/// 故障阻止 API 提供由主库支撑的服务。
pub async fn connect(config: &AppConfig) -> Result<ControlDatabaseCluster, AppError> {
    let primary_config = &config.database.primary;
    let primary_error = |phase, error| {
        startup_database_error(
            config,
            primary_config,
            "database.primary",
            None,
            phase,
            error,
        )
    };
    let primary = ryframe_db::connection::connect_with_sql_logging(
        primary_config,
        config.database.sql_log_level,
        config.database.sql_slow_threshold_ms,
    )
    .await
    .map_err(|error| primary_error("连接池建立", error))?;
    ryframe_db::connection::ping(&primary)
        .await
        .map_err(|error| primary_error("PING 健康检查", error))?;
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
    for (index, source_config) in config.database.sources.iter().enumerate() {
        let field = format!("database.sources[{index}]");
        let source_error = |phase, error| {
            startup_database_error(
                config,
                &source_config.connection,
                &field,
                Some(&source_config.name),
                phase,
                error,
            )
        };
        let source = ryframe_db::connection::connect_with_sql_logging(
            &source_config.connection,
            config.database.sql_log_level,
            config.database.sql_slow_threshold_ms,
        )
        .await
        .map_err(|error| source_error("连接池建立", error))?;
        ryframe_db::connection::ping(&source)
            .await
            .map_err(|error| source_error("PING 健康检查", error))?;
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

fn startup_database_error(
    config: &AppConfig,
    connection: &DbConnection,
    field: &str,
    name: Option<&str>,
    phase: &str,
    error: AppError,
) -> AppError {
    let host = safe_database_target(&connection.host);
    let database = safe_database_target(&connection.database);
    let name = name.map(safe_database_target).unwrap_or("primary");
    let cause = error.to_string();
    let cause = if cause.contains("-----BEGIN") && cause.contains("PRIVATE KEY-----") {
        "底层错误含私钥材料，原文已隐藏"
    } else {
        &cause
    };
    let message = format!(
        "必需数据库依赖启动失败：阶段={phase}；字段={field}；name={name}；目标=[{host}]:{}/{database}；原因链= {cause} ；\
         本次进程启动已中止，尚未进入服务监听阶段。请核对当前所选配置文件中的 \
         {field}.host、port、database，确认 MySQL 服务监听与网络可达、账号连接及数据库访问权限、\
         tls_mode 与 CA/客户端证书/私钥配置；结合底层原因检查 connect_timeout_secs={}、\
         acquire_timeout_secs={} 和连接池容量，修复后重新启动。超时本身不能证明凭据错误。",
        connection.port, connection.connect_timeout_secs, connection.acquire_timeout_secs,
    );
    AppError::Database(redact_database_diagnostic(config, &message))
}

fn safe_database_target(value: &str) -> &str {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || "._-:%[]$".contains(character))
    {
        value
    } else {
        "<目标格式已隐藏，请核对配置字段>"
    }
}

fn redact_database_diagnostic(config: &AppConfig, message: &str) -> String {
    // DbResultExt 已保留驱动文本；这里遮蔽所有数据库角色的凭据及其实际 URL 编码，
    // 不向日志输出完整连接配置，也不读取 TLS 私钥文件。
    let mut secrets = Vec::new();
    for connection in std::iter::once(&config.database.primary)
        .chain(
            config
                .database
                .replicas
                .iter()
                .map(|replica| &replica.connection),
        )
        .chain(
            config
                .database
                .sources
                .iter()
                .map(|source| &source.connection),
        )
    {
        for value in [
            Some(connection.username.as_str()),
            Some(connection.password.as_str()),
            connection.tls_ca.as_deref(),
            connection.tls_client_cert.as_deref(),
            connection.tls_client_key.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        {
            secrets.push(value.to_owned());
            secrets.push(value.replace('\\', "\\\\").replace('\'', "\\'"));
            secrets.push(value.replace('\'', "''"));
            let escaped = format!("{value:?}");
            secrets.push(escaped[1..escaped.len() - 1].to_owned());
        }
        let url = connection.connection_url();
        if let Some((credentials, _)) = url
            .strip_prefix("mysql://")
            .and_then(|value| value.split_once('@'))
        {
            secrets.extend(
                credentials
                    .split(':')
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            );
        }
        if let Some((_, query)) = url.split_once('?') {
            secrets.extend(
                query
                    .split('&')
                    .filter_map(|part| part.split_once('='))
                    .filter(|(key, _)| matches!(*key, "ssl-ca" | "ssl-cert" | "ssl-key"))
                    .map(|(_, value)| value.to_owned()),
            );
        }
    }
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    secrets.dedup();
    // 先遮蔽完整 URL，避免特殊凭据替换改变 URI 分隔符后漏判连接地址。
    let mut redacted = message
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                "<连接地址已隐藏>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    for secret in secrets {
        redacted = redacted.replace(&secret, "<已隐藏>");
    }
    if redacted.contains("-----BEGIN") && redacted.contains("PRIVATE KEY-----") {
        return "数据库启动诊断包含私钥材料，已隐藏；本次进程启动已中止，尚未进入服务监听阶段；请核对所选配置的数据库 TLS 字段及私钥格式后重新启动".into();
    }
    redacted
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
        .map_err(ReplicaProbeFailure::Schema)?;
    ryframe_db::migration::verify_business_schema(connection)
        .await
        .map_err(|error| {
            ReplicaProbeFailure::Schema(AppError::Internal(format!(
                "副本 {name} 的控制库业务结构校验失败：{error}"
            )))
        })
}

/// 按当前迁移账本校验列、索引和外键，账本落后时同样拒绝路由。
async fn verify_database_node(node: &str, db: &DatabaseConnection) -> Result<(), AppError> {
    ryframe_db::migration::verify(db).await.map_err(|error| {
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
