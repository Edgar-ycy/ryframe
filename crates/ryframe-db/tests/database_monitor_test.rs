mod common;

use ryframe_core::DatabaseMonitor;
use ryframe_db::{DatabaseCluster, SeaOrmDatabaseMonitor};

#[tokio::test]
async fn mysql_monitor_reports_health_and_connection_stats() {
    let db = common::setup_test_db().await;
    let monitor = SeaOrmDatabaseMonitor::new(DatabaseCluster::single(db.connection().clone()));

    assert!(monitor.ping().await);
    assert!(
        monitor
            .active_connections()
            .await
            .is_some_and(|value| value >= 1)
    );
    assert_eq!(
        monitor.topology_health().await,
        ryframe_core::DatabaseTopologyHealth {
            primary_healthy: true,
            replicas: Vec::new(),
            sources: Vec::new(),
        }
    );
}
