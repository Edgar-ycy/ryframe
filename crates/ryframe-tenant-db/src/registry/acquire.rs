use std::{sync::Arc, time::Duration};

use ryframe_config::{DbConnection, TenantDatabaseTargetKind};
use sea_orm::DatabaseConnection;
use tokio::sync::watch;

use super::support::{
    AcquireDecision, close_evicted, evict_expired, evict_lru_idle, reserved_connections,
};
use super::{
    OpenPool, OpeningPool, OpeningStatus, TargetDefinition, TenantDataError,
    TenantDatabaseTargetHealthStatus, TenantDatabaseTargetRegistry,
};

impl TenantDatabaseTargetRegistry {
    /// 取得一个 MySQL 目标池活动租约；control 目标由 Router 直接复用控制库集群。
    pub async fn acquire(
        &self,
        target_key: &str,
    ) -> Result<super::TenantDatabasePoolLease, TenantDataError> {
        self.start_idle_sweeper();
        let target = self.inner.targets.get(target_key).cloned().ok_or_else(|| {
            TenantDataError::UnknownTarget {
                target_key: target_key.into(),
            }
        })?;
        if target.kind != TenantDatabaseTargetKind::Mysql {
            return Err(TenantDataError::InvalidConfiguration(format!(
                "control 目标 {target_key} 必须由组合根复用，不能建立独立池"
            )));
        }

        loop {
            match self.acquire_decision(&target).await? {
                AcquireDecision::Ready(lease) => return Ok(lease),
                AcquireDecision::Wait(mut signal) => loop {
                    let status = *signal.borrow_and_update();
                    match status {
                        OpeningStatus::Ready => break,
                        OpeningStatus::Failed => {
                            return Err(TenantDataError::TargetUnavailable {
                                target_key: target.key.to_string(),
                            });
                        }
                        OpeningStatus::Pending => {
                            if signal.changed().await.is_err() {
                                return Err(TenantDataError::TargetUnavailable {
                                    target_key: target.key.to_string(),
                                });
                            }
                        }
                    }
                },
            }
        }
    }

    fn start_idle_sweeper(&self) {
        if self
            .inner
            .idle_sweeper_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let period = self
            .inner
            .idle_pool_duration
            .div_f32(2.0)
            .clamp(Duration::from_secs(1), Duration::from_secs(60));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let evicted = {
                    let mut state = inner.state.lock().await;
                    evict_expired(&mut state, inner.idle_pool_duration)
                };
                close_evicted(evicted);
            }
        });
    }

    async fn acquire_decision(
        &self,
        target: &TargetDefinition,
    ) -> Result<AcquireDecision, TenantDataError> {
        let mut state = self.inner.state.lock().await;
        let mut evicted = evict_expired(&mut state, self.inner.idle_pool_duration);
        if let Some(pool) = state.pools.get(target.key.as_ref()) {
            let lease = pool.acquire(target.key.clone());
            close_evicted(evicted);
            return Ok(AcquireDecision::Ready(lease));
        }
        if let Some(opening) = state.opening.get(target.key.as_ref()) {
            let signal = opening.signal.subscribe();
            close_evicted(evicted);
            return Ok(AcquireDecision::Wait(signal));
        }
        let requested = target
            .pool_max_connections
            .unwrap_or(self.inner.default_pool_max_connections);
        while state.pools.len() + state.opening.len() >= self.inner.max_open_targets
            || reserved_connections(&state).saturating_add(requested)
                > self.inner.max_total_connections
        {
            let Some(pool) = evict_lru_idle(&mut state) else {
                break;
            };
            evicted.push(pool);
        }
        let open_targets = state.pools.len() + state.opening.len();
        if open_targets >= self.inner.max_open_targets {
            close_evicted(evicted);
            return Err(TenantDataError::PoolCapacityExhausted {
                open_targets,
                max_open_targets: self.inner.max_open_targets,
            });
        }
        let used = reserved_connections(&state);
        if used.saturating_add(requested) > self.inner.max_total_connections {
            close_evicted(evicted);
            return Err(TenantDataError::ConnectionBudgetExhausted {
                used,
                requested,
                limit: self.inner.max_total_connections,
            });
        }
        let (signal, receiver) = watch::channel(OpeningStatus::Pending);
        state.opening.insert(
            target.key.clone(),
            OpeningPool {
                signal: signal.clone(),
                max_connections: requested,
            },
        );
        drop(state);
        close_evicted(evicted);
        let registry = self.clone();
        let target = target.clone();
        tokio::spawn(async move {
            registry.open_target(target, signal).await;
        });
        Ok(AcquireDecision::Wait(receiver))
    }

    async fn open_target(&self, target: TargetDefinition, signal: watch::Sender<OpeningStatus>) {
        let result = self.connect_target(&target).await;
        let mut state = self.inner.state.lock().await;
        let reserved = state
            .opening
            .remove(target.key.as_ref())
            .map(|opening| opening.max_connections)
            .unwrap_or_else(|| {
                target
                    .pool_max_connections
                    .unwrap_or(self.inner.default_pool_max_connections)
            });
        let status = match result {
            Ok(connection) => {
                let pool = Arc::new(OpenPool::new(connection, reserved, target.health.clone()));
                state.pools.insert(target.key.clone(), pool);
                OpeningStatus::Ready
            }
            Err(_) => {
                target
                    .health
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .status = TenantDatabaseTargetHealthStatus::Unavailable;
                OpeningStatus::Failed
            }
        };
        let _ = signal.send(status);
    }

    async fn connect_target(
        &self,
        target: &TargetDefinition,
    ) -> Result<DatabaseConnection, TenantDataError> {
        let mysql = target.mysql.as_ref().ok_or_else(|| {
            TenantDataError::InvalidConfiguration(format!("mysql 目标 {} 缺少连接定义", target.key))
        })?;
        let password = std::env::var(&mysql.password_env).map_err(|_| {
            tracing::warn!(
                target = %target.key,
                "租户数据目标密码凭据不可用"
            );
            TenantDataError::TargetUnavailable {
                target_key: target.key.to_string(),
            }
        })?;
        let pool_max_connections = target
            .pool_max_connections
            .unwrap_or(self.inner.default_pool_max_connections);
        let connection_config = DbConnection {
            host: mysql.host.clone(),
            port: mysql.port,
            database: mysql.database.clone(),
            username: mysql.username.clone(),
            password,
            max_connections: pool_max_connections,
            min_connections: 0,
            acquire_timeout_secs: 10,
            idle_timeout_secs: self.inner.idle_pool_duration.as_secs(),
            max_lifetime_secs: 1800,
            connect_timeout_secs: 10,
            tls_mode: mysql.tls_mode,
            tls_ca: mysql.tls_ca.clone(),
            tls_client_cert: mysql.tls_client_cert.clone(),
            tls_client_key: mysql.tls_client_key.clone(),
        };
        let connection = ryframe_db::connection::connect_with_sql_logging(
            &connection_config,
            self.inner.sql_log_level,
            self.inner.sql_slow_threshold_ms,
        )
        .await
        .map_err(|error| {
            tracing::warn!(target = %target.key, %error, "租户数据目标连接失败");
            TenantDataError::TargetUnavailable {
                target_key: target.key.to_string(),
            }
        })?;
        ryframe_db::connection::ping(&connection)
            .await
            .map_err(|error| {
                tracing::warn!(target = %target.key, %error, "租户数据目标健康检查失败");
                TenantDataError::TargetUnavailable {
                    target_key: target.key.to_string(),
                }
            })?;
        tracing::info!(
            target = %target.key,
            max_connections = pool_max_connections,
            "租户数据目标连接池已建立"
        );
        Ok(connection)
    }
}
