use chrono::Utc;
use ryframe_application::ports::backup::{BackupHealth, BackupHealthCache, BackupRepository};
use ryframe_db::ControlDatabaseCluster;
use ryframe_kernel::AppResult;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub const COLLECTION_INTERVAL: Duration = Duration::from_secs(60);
pub const COLLECTION_TIMEOUT: Duration = Duration::from_secs(10);
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(150);

/// 读取部署汇总备份状态；采集失败独立告警，不把备份故障伪装成 API 未就绪。
pub fn spawn(
    database: ControlDatabaseCluster,
    scope_id: String,
    cache: BackupHealthCache,
    shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let repository = ryframe_db::application_ports::backup::port(database);
    tokio::spawn(run(repository, scope_id, cache, shutdown))
}

async fn run(
    repository: Arc<dyn BackupRepository>,
    scope_id: String,
    cache: BackupHealthCache,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(COLLECTION_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            () = async {
                interval.tick().await;
                refresh(&repository, &scope_id, &cache, COLLECTION_TIMEOUT).await;
            } => {}
        }
    }
}

async fn collect(
    repository: &Arc<dyn BackupRepository>,
    scope_id: &str,
) -> AppResult<BackupHealth> {
    let resources = repository.required_resources().await?;
    let now = repository.database_now().await?;
    repository.health(scope_id, &resources, now).await
}

async fn refresh(
    repository: &Arc<dyn BackupRepository>,
    scope_id: &str,
    cache: &BackupHealthCache,
    timeout: Duration,
) {
    let attempted_at = Utc::now();
    match tokio::time::timeout(timeout, collect(repository, scope_id)).await {
        Ok(Ok(health)) => {
            let collected_at = Utc::now();
            ryframe_adapters::metrics::set_backup_health(&health, collected_at);
            cache.update_success(health, collected_at);
        }
        Ok(Err(error)) => {
            cache.update_failure(attempted_at);
            ryframe_adapters::metrics::set_backup_collector_failed();
            tracing::warn!(%error, "备份与恢复演练状态采集失败");
        }
        Err(_) => {
            cache.update_failure(attempted_at);
            ryframe_adapters::metrics::set_backup_collector_failed();
            tracing::warn!(
                timeout_seconds = timeout.as_secs(),
                "备份与恢复演练状态采集超时"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use ryframe_application::ports::backup::{BackupRecord, BackupTransaction, RestoreRecord};
    use tokio::sync::Notify;

    struct HangingRepository {
        started: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl BackupRepository for HangingRepository {
        async fn database_now(&self) -> AppResult<DateTime<Utc>> {
            unreachable!()
        }

        async fn required_resources(&self) -> AppResult<Vec<String>> {
            self.started.notify_one();
            std::future::pending().await
        }

        async fn begin(&self) -> AppResult<Box<dyn BackupTransaction>> {
            unreachable!()
        }

        async fn backup(&self, _id: &str) -> AppResult<Option<BackupRecord>> {
            unreachable!()
        }

        async fn restore(&self, _id: &str) -> AppResult<Option<RestoreRecord>> {
            unreachable!()
        }

        async fn health(
            &self,
            _scope_id: &str,
            _resources: &[String],
            _now: DateTime<Utc>,
        ) -> AppResult<BackupHealth> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn hung_collection_becomes_unavailable_without_publishing_health() {
        let repository: Arc<dyn BackupRepository> = Arc::new(HangingRepository {
            started: Arc::new(Notify::new()),
        });
        let cache = BackupHealthCache::new(Duration::from_secs(10));
        refresh(&repository, "test-scope", &cache, Duration::from_millis(1)).await;
        let snapshot = cache.snapshot();
        assert_eq!(
            snapshot.status,
            ryframe_application::ports::backup::BackupCollectionStatus::Unavailable
        );
        assert!(snapshot.last_attempt_at.is_some());
        assert!(snapshot.last_success_at.is_none());
        assert!(snapshot.last_valid_health.is_none());
    }

    #[tokio::test]
    async fn shutdown_cancels_a_hung_collection() {
        let started = Arc::new(Notify::new());
        let repository: Arc<dyn BackupRepository> = Arc::new(HangingRepository {
            started: Arc::clone(&started),
        });
        let cache = BackupHealthCache::new(Duration::from_secs(10));
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let collector = tokio::spawn(run(
            repository,
            "test-scope".into(),
            cache.clone(),
            shutdown_receiver,
        ));
        started.notified().await;
        shutdown_sender.send(true).unwrap();
        tokio::time::timeout(Duration::from_millis(100), collector)
            .await
            .expect("采集器必须响应统一关闭信号")
            .unwrap();
        assert_eq!(
            cache.snapshot().status,
            ryframe_application::ports::backup::BackupCollectionStatus::Unknown
        );
    }

    #[test]
    fn collection_schedule_leaves_room_for_one_failure_before_stale() {
        assert!(COLLECTION_TIMEOUT < COLLECTION_INTERVAL);
        assert!(CACHE_MAX_AGE > COLLECTION_INTERVAL * 2);
    }
}
