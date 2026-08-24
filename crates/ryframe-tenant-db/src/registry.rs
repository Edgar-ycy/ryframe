use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use ryframe_config::{
    SqlLogLevel, TenantDataConfig, TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::TenantDataError;

mod acquire;
mod lease;
mod support;

use lease::OpenPool;
pub use lease::TenantDatabasePoolLease;
use support::{TargetHealthRecord, reserved_connections, target_definition};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDatabaseTargetMetadata {
    pub key: String,
    pub display_name: Option<String>,
    pub region: Option<String>,
    pub mode: TenantDatabaseTargetMode,
    pub kind: TenantDatabaseTargetKind,
    pub connected: bool,
    pub pool_max_connections: Option<u32>,
    pub active_leases: usize,
    pub schema_fingerprint: Option<String>,
    pub health: TenantDatabaseTargetHealthStatus,
    pub last_verified_at: Option<SystemTime>,
}

impl TenantDatabaseTargetMetadata {
    /// 返回不泄露配置类型的稳定目标占用模式代码。
    pub const fn mode_code(&self) -> &'static str {
        match self.mode {
            TenantDatabaseTargetMode::Shared => "shared",
            TenantDatabaseTargetMode::Dedicated => "dedicated",
        }
    }

    /// 返回不泄露配置类型的稳定连接来源代码。
    pub const fn kind_code(&self) -> &'static str {
        match self.kind {
            TenantDatabaseTargetKind::Control => "control",
            TenantDatabaseTargetKind::Mysql => "mysql",
        }
    }

    pub const fn is_dedicated(&self) -> bool {
        matches!(self.mode, TenantDatabaseTargetMode::Dedicated)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDatabaseTargetHealthStatus {
    Unknown,
    Verified,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDatabasePoolStats {
    pub reserved_connections: u32,
    pub max_total_connections: u32,
    pub open_targets: usize,
    pub opening_targets: usize,
    pub active_leases: usize,
}

#[derive(Clone)]
struct TargetDefinition {
    key: Arc<str>,
    display_name: Option<String>,
    region: Option<String>,
    mode: TenantDatabaseTargetMode,
    kind: TenantDatabaseTargetKind,
    pool_max_connections: Option<u32>,
    mysql: Option<MysqlTargetDefinition>,
    health: Arc<Mutex<TargetHealthRecord>>,
}

impl fmt::Debug for TargetDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TargetDefinition")
            .field("key", &self.key)
            .field("mode", &self.mode)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct MysqlTargetDefinition {
    host: String,
    port: u16,
    database: String,
    username: String,
    password_env: String,
    tls_mode: ryframe_config::DbTlsMode,
    tls_ca: Option<String>,
    tls_client_cert: Option<String>,
    tls_client_key: Option<String>,
}

#[derive(Default)]
struct RegistryState {
    pools: HashMap<Arc<str>, Arc<OpenPool>>,
    opening: HashMap<Arc<str>, OpeningPool>,
}

struct OpeningPool {
    signal: watch::Sender<OpeningStatus>,
    max_connections: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpeningStatus {
    Pending,
    Ready,
    Failed,
}

struct RegistryInner {
    default_target: Arc<str>,
    targets: HashMap<Arc<str>, TargetDefinition>,
    state: AsyncMutex<RegistryState>,
    max_open_targets: usize,
    max_total_connections: u32,
    default_pool_max_connections: u32,
    idle_pool_duration: Duration,
    sql_log_level: SqlLogLevel,
    sql_slow_threshold_ms: u64,
    control_schema_verified: AtomicBool,
    idle_sweeper_started: AtomicBool,
}

impl fmt::Debug for RegistryInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryInner")
            .field("default_target", &self.default_target)
            .field("targets", &self.targets)
            .field("max_open_targets", &self.max_open_targets)
            .field("max_total_connections", &self.max_total_connections)
            .field(
                "default_pool_max_connections",
                &self.default_pool_max_connections,
            )
            .field("idle_pool_duration", &self.idle_pool_duration)
            .finish_non_exhaustive()
    }
}

/// 已批准租户数据目标及其延迟连接池注册表。
#[derive(Clone, Debug)]
pub struct TenantDatabaseTargetRegistry {
    inner: Arc<RegistryInner>,
}

impl TenantDatabaseTargetRegistry {
    pub fn new(
        config: &TenantDataConfig,
        sql_log_level: SqlLogLevel,
        sql_slow_threshold_ms: u64,
    ) -> Result<Self, TenantDataError> {
        config
            .validate()
            .map_err(TenantDataError::InvalidConfiguration)?;
        let default_pool_max_connections =
            config.max_total_connections / config.max_open_targets as u32;
        let targets = config
            .normalized_targets()
            .iter()
            .map(|target| target_definition(target, default_pool_max_connections))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                default_target: Arc::from(config.default_target.as_str()),
                targets,
                state: AsyncMutex::new(RegistryState::default()),
                max_open_targets: config.max_open_targets,
                max_total_connections: config.max_total_connections,
                default_pool_max_connections,
                idle_pool_duration: Duration::from_secs(config.idle_pool_secs),
                sql_log_level,
                sql_slow_threshold_ms,
                control_schema_verified: AtomicBool::new(false),
                idle_sweeper_started: AtomicBool::new(false),
            }),
        })
    }

    pub fn default_target(&self) -> &str {
        &self.inner.default_target
    }

    pub fn contains(&self, target_key: &str) -> bool {
        self.inner.targets.contains_key(target_key)
    }

    pub fn target_mode(&self, target_key: &str) -> Option<TenantDatabaseTargetMode> {
        self.inner.targets.get(target_key).map(|target| target.mode)
    }

    pub fn target_kind(&self, target_key: &str) -> Option<TenantDatabaseTargetKind> {
        self.inner.targets.get(target_key).map(|target| target.kind)
    }

    /// 返回供上层用例持久化和生成指纹使用的稳定模式代码。
    pub fn target_mode_code(&self, target_key: &str) -> Option<&'static str> {
        self.target_mode(target_key).map(|mode| match mode {
            TenantDatabaseTargetMode::Shared => "shared",
            TenantDatabaseTargetMode::Dedicated => "dedicated",
        })
    }

    /// 返回供上层用例持久化和生成指纹使用的稳定来源代码。
    pub fn target_kind_code(&self, target_key: &str) -> Option<&'static str> {
        self.target_kind(target_key).map(|kind| match kind {
            TenantDatabaseTargetKind::Control => "control",
            TenantDatabaseTargetKind::Mysql => "mysql",
        })
    }

    pub fn target_is_dedicated(&self, target_key: &str) -> Option<bool> {
        self.target_mode(target_key)
            .map(|mode| mode == TenantDatabaseTargetMode::Dedicated)
    }

    pub fn target_health(&self, target_key: &str) -> Option<TenantDatabaseTargetHealthStatus> {
        self.inner.targets.get(target_key).map(|target| {
            target
                .health
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .status
        })
    }

    pub fn target_health_is_stale(&self, target_key: &str, max_age: Duration) -> bool {
        self.inner.targets.get(target_key).is_none_or(|target| {
            let health = *target
                .health
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            health.status != TenantDatabaseTargetHealthStatus::Verified
                || health
                    .last_verified_at
                    .and_then(|verified| verified.elapsed().ok())
                    .is_none_or(|age| age >= max_age)
        })
    }

    pub fn len(&self) -> usize {
        self.inner.targets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.targets.is_empty()
    }

    pub fn target_keys(&self) -> Vec<String> {
        let mut keys = self
            .inner
            .targets
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    /// 返回不含地址、数据库名、用户名、密码环境变量名或 TLS 路径的安全元数据。
    pub async fn metadata(&self) -> Vec<TenantDatabaseTargetMetadata> {
        let state = self.inner.state.lock().await;
        let mut targets = self
            .inner
            .targets
            .values()
            .map(|target| {
                let pool = state.pools.get(target.key.as_ref());
                let health = *target
                    .health
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                TenantDatabaseTargetMetadata {
                    key: target.key.to_string(),
                    display_name: target.display_name.clone(),
                    region: target.region.clone(),
                    mode: target.mode,
                    kind: target.kind,
                    connected: target.kind == TenantDatabaseTargetKind::Control || pool.is_some(),
                    pool_max_connections: target.pool_max_connections,
                    active_leases: pool
                        .map(|pool| pool.leases.load(Ordering::Acquire))
                        .unwrap_or(0),
                    // Health=Verified is set only after a complete, current
                    // tenant-data schema verification. A force probe does not
                    // populate the pool-lifetime OnceCell, so key the public
                    // fingerprint off the authoritative health record instead.
                    schema_fingerprint: (health.status
                        == TenantDatabaseTargetHealthStatus::Verified)
                        .then(|| crate::migration::TENANT_DATA_SCHEMA_FINGERPRINT.to_owned()),
                    health: health.status,
                    last_verified_at: health.last_verified_at,
                }
            })
            .collect::<Vec<_>>();
        targets.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        targets
    }

    pub fn mark_control_schema_verified(&self) {
        self.inner
            .control_schema_verified
            .store(true, Ordering::Release);
        self.mark_target_verified("shared-control");
    }

    pub fn mark_target_verified(&self, target_key: &str) {
        if let Some(target) = self.inner.targets.get(target_key) {
            let mut health = target
                .health
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            health.status = TenantDatabaseTargetHealthStatus::Verified;
            health.last_verified_at = Some(SystemTime::now());
        }
    }

    pub fn mark_control_schema_unavailable(&self) {
        self.mark_target_unavailable("shared-control");
    }

    pub fn mark_target_unavailable(&self, target_key: &str) {
        if let Some(target) = self.inner.targets.get(target_key) {
            let mut health = target
                .health
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            health.status = TenantDatabaseTargetHealthStatus::Unavailable;
            health.last_verified_at = Some(SystemTime::now());
        }
    }

    /// 返回当前原子预留的独立池连接预算和打开/活动数量。
    pub async fn pool_stats(&self) -> TenantDatabasePoolStats {
        let state = self.inner.state.lock().await;
        TenantDatabasePoolStats {
            reserved_connections: reserved_connections(&state),
            max_total_connections: self.inner.max_total_connections,
            open_targets: state.pools.len(),
            opening_targets: state.opening.len(),
            active_leases: state
                .pools
                .values()
                .map(|pool| pool.leases.load(Ordering::Acquire))
                .sum(),
        }
    }
}
