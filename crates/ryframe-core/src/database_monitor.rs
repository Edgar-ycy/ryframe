use async_trait::async_trait;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseNodeHealth {
    pub name: String,
    pub healthy: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseTopologyHealth {
    pub primary_healthy: bool,
    pub replicas: Vec<DatabaseNodeHealth>,
    pub sources: Vec<DatabaseNodeHealth>,
}

#[async_trait]
pub trait DatabaseMonitor: Send + Sync {
    async fn ping(&self) -> bool;

    async fn active_connections(&self) -> Option<i64>;

    async fn topology_health(&self) -> DatabaseTopologyHealth;
}
