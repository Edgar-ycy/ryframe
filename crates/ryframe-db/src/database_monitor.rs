use async_trait::async_trait;
use ryframe_core::{DatabaseMonitor, DatabaseTopologyHealth};
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

#[derive(Debug, FromQueryResult)]
struct ActiveConnectionRow {
    value: i64,
}

pub struct SeaOrmDatabaseMonitor {
    database: crate::DatabaseCluster,
}

impl SeaOrmDatabaseMonitor {
    pub fn new(database: crate::DatabaseCluster) -> Self {
        Self { database }
    }
}

#[async_trait]
impl DatabaseMonitor for SeaOrmDatabaseMonitor {
    async fn ping(&self) -> bool {
        let health = self.database.health().await;
        health.primary_healthy
            && health.replicas.iter().all(|replica| replica.healthy)
            && health.sources.iter().all(|source| source.healthy)
    }

    async fn active_connections(&self) -> Option<i64> {
        let db = self.database.write();
        let backend = db.get_database_backend();
        if backend != DatabaseBackend::MySql {
            return None;
        }
        let sql = "SELECT CAST(VARIABLE_VALUE AS SIGNED) AS value \
                   FROM performance_schema.global_status \
                   WHERE VARIABLE_NAME = 'THREADS_CONNECTED'";

        ActiveConnectionRow::find_by_statement(Statement::from_sql_and_values(backend, sql, []))
            .one(db)
            .await
            .ok()
            .flatten()
            .map(|row| row.value)
    }

    async fn topology_health(&self) -> DatabaseTopologyHealth {
        self.database.health().await
    }
}
