use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use ryframe_config::{
    DbConnection, SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig,
    TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use sea_orm::DatabaseConnection;
use tokio::sync::{Mutex as AsyncMutex, OnceCell, watch};

use crate::TenantDataError;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDatabaseTargetHealthStatus {
    Unknown,
    Verified,
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
struct TargetHealthRecord {
    status: TenantDatabaseTargetHealthStatus,
    last_verified_at: Option<SystemTime>,
}

impl Default for TargetHealthRecord {
    fn default() -> Self {
        Self {
            status: TenantDatabaseTargetHealthStatus::Unknown,
            last_verified_at: None,
        }
    }
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

struct OpenPool {
    connection: DatabaseConnection,
    max_connections: u32,
    leases: AtomicUsize,
    last_used: Mutex<Instant>,
    schema_verification: OnceCell<Result<(), TenantDataError>>,
    fresh_schema_verification: AsyncMutex<()>,
    health: Arc<Mutex<TargetHealthRecord>>,
}

impl OpenPool {
    fn new(
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

    fn acquire(self: &Arc<Self>, target_key: Arc<str>) -> TenantDatabasePoolLease {
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

    fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(
            *self
                .last_used
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn is_idle(&self) -> bool {
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
    /// 校验失败会 fail-closed 并缓存到该池被 LRU 驱逐为止。
    pub async fn ensure_schema_verified(&self) -> Result<(), TenantDataError> {
        let result = self
            .pool
            .schema_verification
            .get_or_init(|| async {
                crate::migration::verify_mysql_target(&self.pool.connection)
                    .await
                    .map_err(|error| {
                        tracing::warn!(
                            target = %self.target_key,
                            %error,
                            "租户数据目标 migration ledger 或 schema 指纹不兼容"
                        );
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

    /// 对已打开池执行实时 ping 与完整 schema 校验；互斥同一目标的并发探测，
    /// 不把首次 OnceCell 当作长期健康证明。
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
            crate::migration::verify_mysql_target_for_catalog(
                &self.pool.connection,
                catalog,
            )
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

    /// 取得一个 MySQL 目标池活动租约；control 目标由 Router 直接复用控制库集群。
    pub async fn acquire(
        &self,
        target_key: &str,
    ) -> Result<TenantDatabasePoolLease, TenantDataError> {
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
        if self.inner.idle_sweeper_started.swap(true, Ordering::AcqRel) {
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

enum AcquireDecision {
    Ready(TenantDatabasePoolLease),
    Wait(watch::Receiver<OpeningStatus>),
}

fn target_definition(
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

fn reserved_connections(state: &RegistryState) -> u32 {
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

fn evict_expired(
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

fn evict_lru_idle(state: &mut RegistryState) -> Option<(Arc<str>, Arc<OpenPool>)> {
    let now = Instant::now();
    let candidate = state
        .pools
        .iter()
        .filter(|(_, pool)| pool.is_idle())
        .max_by_key(|(_, pool)| pool.idle_for(now))
        .map(|(key, _)| key.clone());
    candidate.and_then(|key| state.pools.remove(key.as_ref()).map(|pool| (key, pool)))
}

fn close_evicted(pools: Vec<(Arc<str>, Arc<OpenPool>)>) {
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
