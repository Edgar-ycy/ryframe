use std::{sync::Arc, time::Duration};

use ryframe_core::{DatabaseMonitor, RedisClient};
use ryframe_monitor::DependencyHealthCache;
use ryframe_service::system::FileService;
use tokio::{sync::watch, task::JoinHandle};

pub const PROBE_INTERVAL: Duration = Duration::from_secs(5);
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(15);

/// 启动后台依赖探测，供 HTTP 就绪端点读取内存快照。
pub fn spawn(
    database: Arc<dyn DatabaseMonitor>,
    redis: Option<RedisClient>,
    file_service: Option<Arc<FileService>>,
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
                        file_service.as_deref(),
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
    file_service: Option<&FileService>,
    cache: &DependencyHealthCache,
) {
    let mysql = tokio::time::timeout(PROBE_TIMEOUT, database.ping());
    let redis = tokio::time::timeout(PROBE_TIMEOUT, async {
        match redis {
            Some(redis) => redis.ping().await.is_ok(),
            None => false,
        }
    });
    let object_storage = async {
        match file_service {
            Some(file_service) => matches!(
                tokio::time::timeout(PROBE_TIMEOUT, file_service.check_storage()).await,
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
    if !mysql_ok {
        ryframe_middleware::metrics::record_readiness_failure("mysql");
    }
    if !redis_reachable && cache.redis_required() {
        ryframe_middleware::metrics::record_readiness_failure("redis");
    }
    ryframe_middleware::metrics::set_redis_degraded_state("readiness", !redis_reachable);
    if !object_storage_ok && cache.object_storage_required() {
        ryframe_middleware::metrics::record_readiness_failure("object_storage");
    }
}
