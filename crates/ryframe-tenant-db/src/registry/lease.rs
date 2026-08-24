use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use sea_orm::DatabaseConnection;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use super::{TargetHealthRecord, TenantDataError, TenantDatabaseTargetHealthStatus};

pub(super) struct OpenPool {
    pub(super) connection: DatabaseConnection,
    pub(super) max_connections: u32,
    pub(super) leases: AtomicUsize,
    last_used: Mutex<Instant>,
    schema_verification: OnceCell<Result<(), TenantDataError>>,
    fresh_schema_verification: AsyncMutex<()>,
    pub(super) health: Arc<Mutex<TargetHealthRecord>>,
}

impl OpenPool {
    pub(super) fn new(
        connection: DatabaseConnection,
        max_connections: u32,
        health: Arc<Mutex<TargetHealthRecord>>,
    ) -> Self {
        Self {
            connection,
            max_connections,
            leases: AtomicUsize::new(0),
            last_used: Mutex::new(Instant::now()),
            schema_verification: OnceCell::new(),
            fresh_schema_verification: AsyncMutex::new(()),
            health,
        }
    }

    pub(super) fn acquire(self: &Arc<Self>, target_key: Arc<str>) -> TenantDatabasePoolLease {
        self.leases.fetch_add(1, Ordering::AcqRel);
        self.touch();
        TenantDatabasePoolLease {
            target_key,
            pool: self.clone(),
        }
    }

    fn touch(&self) {
        *self
            .last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    pub(super) fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(
            *self
                .last_used
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    pub(super) fn is_idle(&self) -> bool {
        self.leases.load(Ordering::Acquire) == 0
    }
}

/// 持有一个目标池的活动引用；用于阻止 LRU 在 Session 或迁移仍使用连接时回收池。
pub struct TenantDatabasePoolLease {
    target_key: Arc<str>,
    pool: Arc<OpenPool>,
}

impl TenantDatabasePoolLease {
    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub fn connection(&self) -> &DatabaseConnection {
        &self.pool.connection
    }

    /// 每个实际连接池生命周期只执行一次完整 schema/ledger 校验；并发首次使用共享结果。
    pub async fn ensure_schema_verified(&self) -> Result<(), TenantDataError> {
        let result = self
            .pool
            .schema_verification
            .get_or_init(|| async {
                crate::migration::verify_mysql_target(&self.pool.connection)
                    .await
                    .map_err(|error| {
                        tracing::warn!(target = %self.target_key, %error, "租户数据目标 schema 不兼容");
                        TenantDataError::TargetUnavailable {
                            target_key: self.target_key.to_string(),
                        }
                    })
            })
            .await
            .clone();
        let mut health = self
            .pool
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(()) => {
                health.status = TenantDatabaseTargetHealthStatus::Verified;
                health.last_verified_at = Some(SystemTime::now());
                Ok(())
            }
            Err(error) => {
                health.status = TenantDatabaseTargetHealthStatus::Unavailable;
                Err(error)
            }
        }
    }

    pub async fn verify_schema_now_for_catalog(
        &self,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let _guard = self.pool.fresh_schema_verification.lock().await;
        self.verify_schema_locked(catalog).await
    }

    pub async fn verify_schema_if_stale_for_catalog(
        &self,
        catalog: &crate::migration::TenantDataCatalog,
        max_age: Duration,
    ) -> Result<bool, TenantDataError> {
        let _guard = self.pool.fresh_schema_verification.lock().await;
        let health = *self
            .pool
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if health.status == TenantDatabaseTargetHealthStatus::Verified
            && health
                .last_verified_at
                .and_then(|verified| verified.elapsed().ok())
                .is_some_and(|age| age < max_age)
        {
            return Ok(false);
        }
        self.verify_schema_locked(catalog).await.map(|()| true)
    }

    async fn verify_schema_locked(
        &self,
        catalog: &crate::migration::TenantDataCatalog,
    ) -> Result<(), TenantDataError> {
        let result = async {
            ryframe_db::connection::ping(&self.pool.connection)
                .await
                .map_err(|_| TenantDataError::TargetUnavailable {
                    target_key: self.target_key.to_string(),
                })?;
            crate::migration::verify_mysql_target_for_catalog(&self.pool.connection, catalog)
                .await
                .map_err(|error| {
                    tracing::warn!(target = %self.target_key, %error, "租户数据目标实时 schema 校验失败");
                    TenantDataError::TargetUnavailable {
                        target_key: self.target_key.to_string(),
                    }
                })
        }
        .await;
        let mut health = self
            .pool
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        health.last_verified_at = Some(SystemTime::now());
        health.status = if result.is_ok() {
            TenantDatabaseTargetHealthStatus::Verified
        } else {
            TenantDatabaseTargetHealthStatus::Unavailable
        };
        result
    }

    pub fn is_schema_verified(&self) -> bool {
        matches!(self.pool.schema_verification.get(), Some(Ok(())))
    }
}

impl Clone for TenantDatabasePoolLease {
    fn clone(&self) -> Self {
        self.pool.leases.fetch_add(1, Ordering::AcqRel);
        self.pool.touch();
        Self {
            target_key: self.target_key.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl fmt::Debug for TenantDatabasePoolLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TenantDatabasePoolLease")
            .field("target_key", &self.target_key)
            .finish_non_exhaustive()
    }
}

impl Drop for TenantDatabasePoolLease {
    fn drop(&mut self) {
        self.pool.touch();
        self.pool.leases.fetch_sub(1, Ordering::AcqRel);
    }
}
