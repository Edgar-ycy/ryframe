use std::{
    fmt,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use futures_util::future::join_all;
use sea_orm::DatabaseConnection;

const REPLICA_FAILURE_THRESHOLD: usize = 3;
const REPLICA_RECOVERY_THRESHOLD: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseNodeHealth {
    pub name: String,
    pub healthy: bool,
    pub consecutive_failures: usize,
    pub consecutive_successes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseTopologyHealth {
    pub primary_healthy: bool,
    pub replicas: Vec<DatabaseNodeHealth>,
    pub sources: Vec<DatabaseNodeHealth>,
}

/// 数据库读取的一致性策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadConsistency {
    /// 有可用健康副本时使用副本，否则回退到主库。
    Eventual,
    /// 对授权敏感和写后读取路径始终使用主库。
    Strong,
}

/// 为查询选定的节点类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseNodeKind {
    Primary,
    Replica,
}

impl DatabaseNodeKind {
    /// 返回用于监控标签的固定节点类型。
    pub const fn metric_label(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Replica => "replica",
        }
    }
}

/// 数据库读路由的有限原因集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseReadSelectionReason {
    /// 调用方显式要求强一致性。
    Strong,
    /// 最终一致性读取命中了健康副本。
    Replica,
    /// 没有可用副本，最终一致性读取回退主库。
    Fallback,
}

impl DatabaseReadSelectionReason {
    /// 返回用于监控标签的固定路由原因。
    pub const fn metric_label(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Replica => "replica",
            Self::Fallback => "fallback",
        }
    }
}

/// 由应用层实现的数据库监控观察者，避免数据访问层反向依赖监控实现。
pub trait DatabaseMetricsObserver: fmt::Debug + Send + Sync {
    /// 更新一个已配置节点的当前路由健康状态。
    fn set_node_health(&self, kind: DatabaseNodeKind, name: &str, healthy: bool);

    /// 记录一次读路由选择。
    fn record_read_selection(&self, target: DatabaseNodeKind, reason: DatabaseReadSelectionReason);

    /// 记录一次最终一致性读取回退主库。
    fn record_read_fallback(&self);
}

type NodeHealthCallback = dyn Fn(DatabaseNodeKind, &str, bool) + Send + Sync;
type ReadSelectionCallback = dyn Fn(DatabaseNodeKind, DatabaseReadSelectionReason) + Send + Sync;
type ReadFallbackCallback = dyn Fn() + Send + Sync;

/// 使用回调把数据库事件适配到应用层监控的观察者实现。
#[derive(Clone)]
pub struct CallbackDatabaseMetricsObserver {
    on_node_health: Arc<NodeHealthCallback>,
    on_read_selection: Arc<ReadSelectionCallback>,
    on_read_fallback: Arc<ReadFallbackCallback>,
}

impl CallbackDatabaseMetricsObserver {
    /// 创建由应用层回调驱动的监控观察者。
    pub fn new(
        on_node_health: Arc<NodeHealthCallback>,
        on_read_selection: Arc<ReadSelectionCallback>,
        on_read_fallback: Arc<ReadFallbackCallback>,
    ) -> Self {
        Self {
            on_node_health,
            on_read_selection,
            on_read_fallback,
        }
    }
}

impl fmt::Debug for CallbackDatabaseMetricsObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackDatabaseMetricsObserver")
            .finish_non_exhaustive()
    }
}

impl DatabaseMetricsObserver for CallbackDatabaseMetricsObserver {
    fn set_node_health(&self, kind: DatabaseNodeKind, name: &str, healthy: bool) {
        (self.on_node_health)(kind, name, healthy);
    }

    fn record_read_selection(&self, target: DatabaseNodeKind, reason: DatabaseReadSelectionReason) {
        (self.on_read_selection)(target, reason);
    }

    fn record_read_fallback(&self) {
        (self.on_read_fallback)();
    }
}

/// 依据显式一致性策略选定的连接。
#[derive(Clone, Debug)]
pub struct SelectedDatabase {
    pub node_name: Arc<str>,
    pub kind: DatabaseNodeKind,
    pub connection: DatabaseConnection,
}

/// 由应用集群管理的具名只读副本槽位。
///
/// 连接池以可替换槽位保存：副本暂时不可达时仍保留配置名称和健康状态，后台监督器
/// 可以在不重启 API 进程的情况下建立新连接池。
#[derive(Debug)]
struct ReplicaNode {
    name: Arc<str>,
    connection: RwLock<Option<DatabaseConnection>>,
    healthy: AtomicBool,
    consecutive_failures: AtomicUsize,
    consecutive_successes: AtomicUsize,
}

impl ReplicaNode {
    fn new(name: String, connection: Option<DatabaseConnection>, healthy: bool) -> Self {
        let initially_healthy = healthy && connection.is_some();
        Self {
            name: Arc::from(name),
            connection: RwLock::new(connection),
            healthy: AtomicBool::new(initially_healthy),
            consecutive_failures: AtomicUsize::new(0),
            consecutive_successes: AtomicUsize::new(0),
        }
    }

    fn connection(&self) -> Option<DatabaseConnection> {
        self.connection
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace_connection(&self, connection: DatabaseConnection) {
        *self
            .connection
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(connection);
    }

    fn clear_connection(&self) {
        *self
            .connection
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.healthy.store(false, Ordering::Release);
        self.consecutive_successes.store(0, Ordering::Release);
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire) && self.connection().is_some()
    }

    fn record_probe(&self, reachable: bool) {
        if reachable {
            self.consecutive_failures.store(0, Ordering::Release);
            let successes = increment_streak(&self.consecutive_successes);
            if !self.is_healthy() && successes >= REPLICA_RECOVERY_THRESHOLD {
                self.healthy
                    .store(self.connection().is_some(), Ordering::Release);
            }
            return;
        }

        self.consecutive_successes.store(0, Ordering::Release);
        let failures = increment_streak(&self.consecutive_failures);
        if failures >= REPLICA_FAILURE_THRESHOLD {
            self.healthy.store(false, Ordering::Release);
        }
    }

    fn health(&self) -> DatabaseNodeHealth {
        DatabaseNodeHealth {
            name: self.name.to_string(),
            healthy: self.is_healthy(),
            consecutive_failures: self.consecutive_failures.load(Ordering::Acquire),
            consecutive_successes: self.consecutive_successes.load(Ordering::Acquire),
        }
    }
}

fn increment_streak(streak: &AtomicUsize) -> usize {
    streak
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.saturating_add(1))
        })
        .unwrap_or(usize::MAX)
        .saturating_add(1)
}

/// 具名业务数据源不参与副本自动恢复，保持为启动时建立的固定连接池。
#[derive(Debug)]
struct DatabaseNode {
    name: Arc<str>,
    connection: DatabaseConnection,
}

impl DatabaseNode {
    fn new(name: String, connection: DatabaseConnection) -> Self {
        Self {
            name: Arc::from(name),
            connection,
        }
    }
}

#[derive(Debug)]
struct ControlDatabaseClusterInner {
    primary: DatabaseConnection,
    replicas: Box<[ReplicaNode]>,
    sources: Box<[DatabaseNode]>,
    next_replica: AtomicUsize,
    metrics_observer: RwLock<Option<Arc<dyn DatabaseMetricsObserver>>>,
}

/// 共享的主库、副本及具名业务数据源连接池。
///
/// 命令始终使用 [`ControlDatabaseCluster::write`]。查询只能通过
/// [`ControlDatabaseCluster::select_read`] 显式声明需要强一致性还是最终一致性；最终一致性读取
/// 会按轮询顺序选择健康副本，并在没有可用副本时回退到主库。
/// 异构业务数据源只能通过 [`ControlDatabaseCluster::source`] 获取，绝不参与自动路由。
#[derive(Clone, Debug)]
pub struct ControlDatabaseCluster {
    inner: Arc<ControlDatabaseClusterInner>,
}

impl ControlDatabaseCluster {
    /// 使用可替换副本连接槽位构建集群。
    ///
    /// `None` 表示该副本已经配置但当前无可用连接；它会保留在拓扑和指标中，等待
    /// 组合根的后台监督器按退避策略重连。
    pub fn with_sources_and_replica_slots(
        primary: DatabaseConnection,
        replicas: impl IntoIterator<Item = (String, Option<DatabaseConnection>, bool)>,
        sources: impl IntoIterator<Item = (String, DatabaseConnection)>,
    ) -> Self {
        let replicas = replicas
            .into_iter()
            .map(|(name, connection, healthy)| ReplicaNode::new(name, connection, healthy))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let sources = collect_nodes(sources);

        Self {
            inner: Arc::new(ControlDatabaseClusterInner {
                primary,
                replicas,
                sources,
                next_replica: AtomicUsize::new(0),
                metrics_observer: RwLock::new(None),
            }),
        }
    }

    pub fn single(primary: DatabaseConnection) -> Self {
        Self::with_sources_and_replica_slots(primary, std::iter::empty(), std::iter::empty())
    }

    /// 安装应用层提供的低基数监控观察者，并同步当前已配置节点状态。
    pub fn set_metrics_observer(&self, observer: Arc<dyn DatabaseMetricsObserver>) {
        let mut stored = self
            .inner
            .metrics_observer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *stored = Some(observer.clone());
        drop(stored);

        observer.set_node_health(DatabaseNodeKind::Primary, "primary", true);
        for replica in self.inner.replicas.iter() {
            observer.set_node_health(
                DatabaseNodeKind::Replica,
                &replica.name,
                replica.is_healthy(),
            );
        }
    }

    /// 为命令和一致性敏感读取返回主库连接池。
    pub fn write(&self) -> &DatabaseConnection {
        &self.inner.primary
    }

    /// 按显式一致性策略选择克隆的连接池。
    pub fn select_read(&self, consistency: ReadConsistency) -> SelectedDatabase {
        match consistency {
            ReadConsistency::Strong => {
                self.record_read_selection(
                    DatabaseNodeKind::Primary,
                    DatabaseReadSelectionReason::Strong,
                );
                SelectedDatabase {
                    node_name: Arc::from("primary"),
                    kind: DatabaseNodeKind::Primary,
                    connection: self.inner.primary.clone(),
                }
            }
            ReadConsistency::Eventual => match self.select_read_replica() {
                Some((name, connection)) => {
                    self.record_read_selection(
                        DatabaseNodeKind::Replica,
                        DatabaseReadSelectionReason::Replica,
                    );
                    SelectedDatabase {
                        node_name: name,
                        kind: DatabaseNodeKind::Replica,
                        connection,
                    }
                }
                None => {
                    self.record_read_selection(
                        DatabaseNodeKind::Primary,
                        DatabaseReadSelectionReason::Fallback,
                    );
                    self.record_read_fallback();
                    SelectedDatabase {
                        node_name: Arc::from("primary"),
                        kind: DatabaseNodeKind::Primary,
                        connection: self.inner.primary.clone(),
                    }
                }
            },
        }
    }

    /// 返回具名的异构业务数据源。
    pub fn source(&self, name: &str) -> Option<&DatabaseConnection> {
        self.inner
            .sources
            .iter()
            .find(|source| source.name.as_ref() == name)
            .map(|source| &source.connection)
    }

    pub fn replica_count(&self) -> usize {
        self.inner.replicas.len()
    }

    pub fn source_count(&self) -> usize {
        self.inner.sources.len()
    }

    pub fn replica_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner
            .replicas
            .iter()
            .map(|replica| replica.name.as_ref())
    }

    pub fn source_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner.sources.iter().map(|source| source.name.as_ref())
    }

    /// 返回当前已建立连接的副本快照。
    pub fn replicas(&self) -> Vec<(String, DatabaseConnection)> {
        self.inner
            .replicas
            .iter()
            .filter_map(|replica| {
                replica
                    .connection()
                    .map(|connection| (replica.name.to_string(), connection))
            })
            .collect()
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = (&str, &DatabaseConnection)> + Clone {
        node_connections(&self.inner.sources)
    }

    /// 读取一个副本槽位当前持有的连接池快照。
    pub fn replica_connection(&self, name: &str) -> Option<DatabaseConnection> {
        self.inner
            .replicas
            .iter()
            .find(|replica| replica.name.as_ref() == name)
            .and_then(ReplicaNode::connection)
    }

    /// 将新建立的连接池安装到已配置副本槽位；连接仍需通过连续成功探测后才能路由。
    pub fn replace_replica_connection(&self, name: &str, connection: DatabaseConnection) -> bool {
        let Some(replica) = self
            .inner
            .replicas
            .iter()
            .find(|replica| replica.name.as_ref() == name)
        else {
            return false;
        };
        replica.replace_connection(connection);
        self.record_node_health(DatabaseNodeKind::Replica, &replica.name, false);
        true
    }

    /// 清除无法继续使用的副本连接池并立即从路由中摘除。
    pub fn clear_replica_connection(&self, name: &str) -> bool {
        let Some(replica) = self
            .inner
            .replicas
            .iter()
            .find(|replica| replica.name.as_ref() == name)
        else {
            return false;
        };
        replica.clear_connection();
        self.record_node_health(DatabaseNodeKind::Replica, &replica.name, false);
        true
    }

    /// 由唯一的后台监督器写入副本探测结果。
    ///
    /// 连续三次失败才摘除可路由副本，连续两次完整成功才重新接流量。请求路径只读取
    /// 快照，不得调用此方法推进阈值。
    pub fn record_replica_probe(&self, name: &str, successful: bool) -> bool {
        let Some(replica) = self
            .inner
            .replicas
            .iter()
            .find(|replica| replica.name.as_ref() == name)
        else {
            return false;
        };
        replica.record_probe(successful && replica.connection().is_some());
        self.record_node_health(
            DatabaseNodeKind::Replica,
            &replica.name,
            replica.is_healthy(),
        );
        true
    }

    /// 返回副本是否可参与自动读路由。
    pub fn replica_is_routable(&self, name: &str) -> Option<bool> {
        self.inner
            .replicas
            .iter()
            .find(|replica| replica.name.as_ref() == name)
            .map(ReplicaNode::is_healthy)
    }

    /// 返回所有已配置副本的可路由健康快照。
    pub fn replica_health(&self) -> Vec<DatabaseNodeHealth> {
        self.inner
            .replicas
            .iter()
            .map(ReplicaNode::health)
            .collect()
    }

    pub async fn health(&self) -> DatabaseTopologyHealth {
        let (primary_healthy, sources) = futures_util::future::join(
            async { crate::connection::ping(self.write()).await.is_ok() },
            node_health(self.sources()),
        )
        .await;

        self.record_node_health(DatabaseNodeKind::Primary, "primary", primary_healthy);

        DatabaseTopologyHealth {
            primary_healthy,
            replicas: self.replica_health(),
            sources,
        }
    }

    fn select_read_replica(&self) -> Option<(Arc<str>, DatabaseConnection)> {
        let replicas = &self.inner.replicas;
        if replicas.is_empty() {
            return None;
        }
        let start = self.inner.next_replica.fetch_add(1, Ordering::Relaxed) % replicas.len();
        (0..replicas.len())
            .map(|offset| &replicas[(start + offset) % replicas.len()])
            .find_map(|replica| {
                replica
                    .is_healthy()
                    .then(|| replica.connection())
                    .flatten()
                    .map(|connection| (replica.name.clone(), connection))
            })
    }

    fn metrics_observer(&self) -> Option<Arc<dyn DatabaseMetricsObserver>> {
        self.inner
            .metrics_observer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record_node_health(&self, kind: DatabaseNodeKind, name: &str, healthy: bool) {
        if let Some(observer) = self.metrics_observer() {
            observer.set_node_health(kind, name, healthy);
        }
    }

    fn record_read_selection(&self, target: DatabaseNodeKind, reason: DatabaseReadSelectionReason) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_read_selection(target, reason);
        }
    }

    fn record_read_fallback(&self) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_read_fallback();
        }
    }
}

fn collect_nodes(
    nodes: impl IntoIterator<Item = (String, DatabaseConnection)>,
) -> Box<[DatabaseNode]> {
    nodes
        .into_iter()
        .map(|(name, connection)| DatabaseNode::new(name, connection))
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn node_connections(
    nodes: &[DatabaseNode],
) -> impl ExactSizeIterator<Item = (&str, &DatabaseConnection)> + Clone {
    nodes
        .iter()
        .map(|node| (node.name.as_ref(), &node.connection))
}

async fn node_health<'a>(
    nodes: impl ExactSizeIterator<Item = (&'a str, &'a DatabaseConnection)>,
) -> Vec<DatabaseNodeHealth> {
    join_all(nodes.map(|(name, connection)| async move {
        DatabaseNodeHealth {
            name: name.to_owned(),
            healthy: crate::connection::ping(connection).await.is_ok(),
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }))
    .await
}

impl From<DatabaseConnection> for ControlDatabaseCluster {
    fn from(connection: DatabaseConnection) -> Self {
        Self::single(connection)
    }
}
