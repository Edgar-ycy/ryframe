use ryframe_core::DatabaseMonitor;
use ryframe_db::{DatabaseCluster, SeaOrmDatabaseMonitor};
use sea_orm::Database;

#[tokio::test]
async fn sqlite_monitor_reports_health_without_vendor_connection_stats() {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    let monitor = SeaOrmDatabaseMonitor::new(DatabaseCluster::single(db));

    assert!(monitor.ping().await);
    assert_eq!(monitor.active_connections().await, None);
    assert_eq!(
        monitor.topology_health().await,
        ryframe_core::DatabaseTopologyHealth {
            primary_healthy: true,
            replicas: Vec::new(),
            sources: Vec::new(),
        }
    );
}
