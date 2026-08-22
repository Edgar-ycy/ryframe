use std::{sync::Arc, time::Duration};

use ryframe_adapters::RedisClient;
use ryframe_api::monitor::{
    DatabaseConnectionCountFuture, DatabaseMonitor, DatabaseNodeHealth, DatabasePingFuture,
    DatabaseTopologyFuture, DatabaseTopologyHealth, DependencyHealthCache, DependencyStatus,
};
use ryframe_application::system::FileService;
use ryframe_db::{ControlDatabaseCluster, SeaOrmDatabaseMonitor};
use tokio::{sync::watch, task::JoinHandle};

pub const PROBE_INTERVAL: Duration = Duration::from_secs(5);
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(15);

struct DatabaseMonitorBridge {
    monitor: SeaOrmDatabaseMonitor,
}

impl DatabaseMonitor for DatabaseMonitorBridge {
    fn ping(&self) -> DatabasePingFuture<'_> {
        Box::pin(self.monitor.ping())
    }

    fn active_connections(&self) -> DatabaseConnectionCountFuture<'_> {
        Box::pin(self.monitor.active_connections())
    }

    fn topology_health(&self) -> DatabaseTopologyFuture<'_> {
        Box::pin(async move {
            let health = self.monitor.topology_health().await;
            DatabaseTopologyHealth {
                primary_healthy: health.primary_healthy,
                replicas: health.replicas.into_iter().map(map_node_health).collect(),
                sources: health.sources.into_iter().map(map_node_health).collect(),
            }
        })
    }
}

fn map_node_health(health: ryframe_db::DatabaseNodeHealth) -> DatabaseNodeHealth {
    DatabaseNodeHealth {
        name: health.name,
        healthy: health.healthy,
        consecutive_failures: health.consecutive_failures,
        consecutive_successes: health.consecutive_successes,
    }
}

pub fn database_monitor(database: ControlDatabaseCluster) -> Arc<dyn DatabaseMonitor> {
    Arc::new(DatabaseMonitorBridge {
        monitor: SeaOrmDatabaseMonitor::new(database),
    })
}

/// 启动后台依赖探测，供 HTTP 就绪端点读取内存快照。
pub fn spawn(
    database: Arc<dyn DatabaseMonitor>,
    redis: Option<RedisClient>,
    file: Option<Arc<FileService>>,
    cache: DependencyHealthCache,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PROBE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    probe_once(
                        database.as_ref(),
                        redis.as_ref(),
                        file.as_deref(),
                        &cache,
                    )
                    .await;
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

async fn probe_once(
    database: &dyn DatabaseMonitor,
    redis: Option<&RedisClient>,
    file: Option<&FileService>,
    cache: &DependencyHealthCache,
) {
    let previous = cache.snapshot();
    let mysql = tokio::time::timeout(PROBE_TIMEOUT, database.ping());
    let redis = tokio::time::timeout(PROBE_TIMEOUT, async {
        match redis {
            Some(redis) => redis.ping().await.is_ok(),
            None => false,
        }
    });
    let object_storage = async {
        match file {
            Some(file) => matches!(
                tokio::time::timeout(PROBE_TIMEOUT, file.check_storage()).await,
                Ok(Ok(()))
            ),
            None => true,
        }
    };
    let (mysql_result, redis_result, object_storage_ok) =
        tokio::join!(mysql, redis, object_storage);
    let mysql_ok = matches!(mysql_result, Ok(true));
    let redis_reachable = matches!(redis_result, Ok(true));

    cache.update(mysql_ok, redis_reachable, object_storage_ok);
    let current = cache.snapshot();
    report_dependency_status("mysql", previous.mysql, current.mysql);
    if cache.redis_required() {
        report_dependency_status("redis", previous.redis, current.redis);
    }
    if cache.object_storage_required() {
        report_dependency_status(
            "object_storage",
            previous.object_storage,
            current.object_storage,
        );
    }
    if !mysql_ok {
        ryframe_adapters::metrics::record_readiness_failure("mysql");
    }
    if !redis_reachable && cache.redis_required() {
        ryframe_adapters::metrics::record_readiness_failure("redis");
    }
    ryframe_adapters::metrics::set_redis_degraded_state("readiness", !redis_reachable);
    if !object_storage_ok && cache.object_storage_required() {
        ryframe_adapters::metrics::record_readiness_failure("object_storage");
    }
}

/// 仅在状态变化时提高日志级别，避免健康探测在依赖稳定时持续刷屏。
fn report_dependency_status(
    dependency: &'static str,
    previous: DependencyStatus,
    current: DependencyStatus,
) {
    if previous == current {
        tracing::trace!(dependency, status = current.as_str(), "就绪依赖状态未变化");
        return;
    }

    match current {
        DependencyStatus::Up => {
            let message = if previous == DependencyStatus::Unknown {
                "就绪依赖探测已就绪"
            } else {
                "就绪依赖已恢复"
            };
            tracing::info!(
                dependency,
                previous_status = previous.as_str(),
                status = current.as_str(),
                "{message}"
            );
        }
        DependencyStatus::Down => {
            tracing::warn!(
                dependency,
                previous_status = previous.as_str(),
                status = current.as_str(),
                "就绪依赖不可用"
            );
        }
        DependencyStatus::Unknown
        | DependencyStatus::OptionalDegraded
        | DependencyStatus::NotRequired => {
            tracing::debug!(
                dependency,
                previous_status = previous.as_str(),
                status = current.as_str(),
                "就绪依赖状态已更新"
            );
        }
    }
}
