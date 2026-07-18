use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ryframe_core::{DatabaseNodeHealth, DatabaseTopologyHealth};
use sea_orm::DatabaseConnection;

/// A named database node owned by the application cluster.
#[derive(Debug)]
struct DatabaseNode {
    name: String,
    connection: DatabaseConnection,
}

#[derive(Debug)]
struct DatabaseClusterInner {
    primary: DatabaseConnection,
    replicas: Box<[DatabaseNode]>,
    sources: Box<[DatabaseNode]>,
    next_replica: AtomicUsize,
}

/// Shared primary, replica and named business-data-source connection pools.
///
/// Commands always use [`DatabaseCluster::write`]. Read-only operations use
/// [`DatabaseCluster::read`], which selects configured replicas in round-robin
/// order and falls back to the primary only when no replica is configured.
/// Heterogeneous business data sources are available only through
/// [`DatabaseCluster::source`] and never participate in automatic routing.
#[derive(Clone, Debug)]
pub struct DatabaseCluster {
    inner: Arc<DatabaseClusterInner>,
}

impl DatabaseCluster {
    pub fn new(
        primary: DatabaseConnection,
        replicas: impl IntoIterator<Item = (String, DatabaseConnection)>,
    ) -> Self {
        Self::with_sources(primary, replicas, std::iter::empty())
    }

    pub fn with_sources(
        primary: DatabaseConnection,
        replicas: impl IntoIterator<Item = (String, DatabaseConnection)>,
        sources: impl IntoIterator<Item = (String, DatabaseConnection)>,
    ) -> Self {
        let replicas = collect_nodes(replicas);
        let sources = collect_nodes(sources);

        Self {
            inner: Arc::new(DatabaseClusterInner {
                primary,
                replicas,
                sources,
                next_replica: AtomicUsize::new(0),
            }),
        }
    }

    pub fn single(primary: DatabaseConnection) -> Self {
        Self::new(primary, std::iter::empty())
    }

    /// Return the primary pool for commands and consistency-sensitive reads.
    pub fn write(&self) -> &DatabaseConnection {
        &self.inner.primary
    }

    /// Return a replica pool for a read-only operation.
    ///
    /// The primary is returned only for a single-node topology.
    pub fn read(&self) -> &DatabaseConnection {
        self.select_read_replica()
            .map_or_else(|| self.write(), |replica| &replica.connection)
    }

    /// Return a named heterogeneous business data source.
    pub fn source(&self, name: &str) -> Option<&DatabaseConnection> {
        self.inner
            .sources
            .iter()
            .find(|source| source.name == name)
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
            .map(|replica| replica.name.as_str())
    }

    pub fn source_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner.sources.iter().map(|source| source.name.as_str())
    }

    pub fn replicas(&self) -> impl ExactSizeIterator<Item = (&str, &DatabaseConnection)> + Clone {
        node_connections(&self.inner.replicas)
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = (&str, &DatabaseConnection)> + Clone {
        node_connections(&self.inner.sources)
    }

    pub async fn health(&self) -> DatabaseTopologyHealth {
        let primary_healthy = crate::connection::ping(self.write()).await.is_ok();
        let replicas = node_health(self.replicas()).await;
        let sources = node_health(self.sources()).await;

        DatabaseTopologyHealth {
            primary_healthy,
            replicas,
            sources,
        }
    }

    fn select_read_replica(&self) -> Option<&DatabaseNode> {
        let replicas = &self.inner.replicas;
        if replicas.is_empty() {
            return None;
        }
        let index = self.inner.next_replica.fetch_add(1, Ordering::Relaxed) % replicas.len();
        Some(&replicas[index])
    }
}

fn collect_nodes(
    nodes: impl IntoIterator<Item = (String, DatabaseConnection)>,
) -> Box<[DatabaseNode]> {
    nodes
        .into_iter()
        .map(|(name, connection)| DatabaseNode { name, connection })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn node_connections(
    nodes: &[DatabaseNode],
) -> impl ExactSizeIterator<Item = (&str, &DatabaseConnection)> + Clone {
    nodes
        .iter()
        .map(|node| (node.name.as_str(), &node.connection))
}

async fn node_health<'a>(
    nodes: impl ExactSizeIterator<Item = (&'a str, &'a DatabaseConnection)>,
) -> Vec<DatabaseNodeHealth> {
    let mut health = Vec::with_capacity(nodes.len());
    for (name, connection) in nodes {
        health.push(DatabaseNodeHealth {
            name: name.to_owned(),
            healthy: crate::connection::ping(connection).await.is_ok(),
        });
    }
    health
}

impl From<DatabaseConnection> for DatabaseCluster {
    fn from(connection: DatabaseConnection) -> Self {
        Self::single(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rotate_over_replicas_and_single_node_falls_back() {
        let cluster = DatabaseCluster::new(
            DatabaseConnection::default(),
            [
                ("replica-a".to_owned(), DatabaseConnection::default()),
                ("replica-b".to_owned(), DatabaseConnection::default()),
            ],
        );

        let selected = (0..3)
            .map(|_| cluster.select_read_replica().unwrap().name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(selected, ["replica-a", "replica-b", "replica-a"]);

        let single = DatabaseCluster::single(DatabaseConnection::default());
        assert!(single.select_read_replica().is_none());
        assert!(std::ptr::eq(single.read(), single.write()));
    }

    #[test]
    fn named_sources_require_explicit_selection() {
        let primary = DatabaseConnection::default();
        let device = DatabaseConnection::default();
        let cluster = DatabaseCluster::with_sources(
            primary,
            std::iter::empty(),
            [("ryframe_device".to_owned(), device)],
        );

        assert!(cluster.source("ryframe_device").is_some());
        assert!(cluster.source("missing").is_none());
    }
}
