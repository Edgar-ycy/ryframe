use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use ryframe_config::{TenantDatabaseTargetConfig, TenantDatabaseTargetKind};
use tokio::sync::watch;

use super::{
    MysqlTargetDefinition, OpenPool, OpeningStatus, RegistryState, TargetDefinition,
    TenantDataError, TenantDatabasePoolLease, TenantDatabaseTargetHealthStatus,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct TargetHealthRecord {
    pub(super) status: TenantDatabaseTargetHealthStatus,
    pub(super) last_verified_at: Option<SystemTime>,
}

impl Default for TargetHealthRecord {
    fn default() -> Self {
        Self {
            status: TenantDatabaseTargetHealthStatus::Unknown,
            last_verified_at: None,
        }
    }
}

pub(super) enum AcquireDecision {
    Ready(TenantDatabasePoolLease),
    Wait(watch::Receiver<OpeningStatus>),
}

pub(super) fn target_definition(
    target: &TenantDatabaseTargetConfig,
    default_pool_max_connections: u32,
) -> Result<(Arc<str>, TargetDefinition), TenantDataError> {
    let key: Arc<str> = Arc::from(target.key.as_str());
    let mysql = if target.kind == TenantDatabaseTargetKind::Mysql {
        Some(MysqlTargetDefinition {
            host: target.host.clone().unwrap_or_default(),
            port: target.port.unwrap_or(3306),
            database: target.database.clone().unwrap_or_default(),
            username: target.username.clone().unwrap_or_default(),
            password_env: target.password_env.clone().unwrap_or_default(),
            tls_mode: target.tls_mode.unwrap_or_default(),
            tls_ca: target.tls_ca.clone(),
            tls_client_cert: target.tls_client_cert.clone(),
            tls_client_key: target.tls_client_key.clone(),
        })
    } else {
        None
    };
    Ok((
        key.clone(),
        TargetDefinition {
            key,
            display_name: target.display_name.clone(),
            region: target.region.clone(),
            mode: target.mode,
            kind: target.kind,
            pool_max_connections: (target.kind == TenantDatabaseTargetKind::Mysql).then_some(
                target
                    .max_connections
                    .unwrap_or(default_pool_max_connections),
            ),
            mysql,
            health: Arc::new(Mutex::new(TargetHealthRecord::default())),
        },
    ))
}

pub(super) fn reserved_connections(state: &RegistryState) -> u32 {
    state
        .pools
        .values()
        .map(|pool| pool.max_connections)
        .chain(
            state
                .opening
                .values()
                .map(|opening| opening.max_connections),
        )
        .fold(0, u32::saturating_add)
}

pub(super) fn evict_expired(
    state: &mut RegistryState,
    idle_duration: Duration,
) -> Vec<(Arc<str>, Arc<OpenPool>)> {
    let now = Instant::now();
    let keys = state
        .pools
        .iter()
        .filter(|(_, pool)| pool.is_idle() && pool.idle_for(now) >= idle_duration)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    keys.into_iter()
        .filter_map(|key| state.pools.remove(key.as_ref()).map(|pool| (key, pool)))
        .collect()
}

pub(super) fn evict_lru_idle(state: &mut RegistryState) -> Option<(Arc<str>, Arc<OpenPool>)> {
    let now = Instant::now();
    let candidate = state
        .pools
        .iter()
        .filter(|(_, pool)| pool.is_idle())
        .max_by_key(|(_, pool)| pool.idle_for(now))
        .map(|(key, _)| key.clone());
    candidate.and_then(|key| state.pools.remove(key.as_ref()).map(|pool| (key, pool)))
}

pub(super) fn close_evicted(pools: Vec<(Arc<str>, Arc<OpenPool>)>) {
    for (target_key, pool) in pools {
        pool.health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status = TenantDatabaseTargetHealthStatus::Unknown;
        tokio::spawn(async move {
            if let Err(error) = pool.connection.clone().close().await {
                tracing::warn!(target = %target_key, %error, "空闲租户数据目标池关闭失败");
            } else {
                tracing::debug!(target = %target_key, "空闲租户数据目标池已关闭");
            }
        });
    }
}
