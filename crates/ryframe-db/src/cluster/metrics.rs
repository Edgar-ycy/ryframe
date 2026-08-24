use std::{fmt, sync::Arc};

use sea_orm::DatabaseConnection;

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
    Strong,
    Replica,
    Fallback,
}

impl DatabaseReadSelectionReason {
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
    fn set_node_health(&self, kind: DatabaseNodeKind, name: &str, healthy: bool);
    fn record_read_selection(&self, target: DatabaseNodeKind, reason: DatabaseReadSelectionReason);
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
