use std::time::Duration;

use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

use crate::DatabaseTopologyHealth;

#[derive(Debug, FromQueryResult)]
struct ActiveConnectionRow {
    value: i64,
}

pub struct SeaOrmDatabaseMonitor {
    database: crate::ControlDatabaseCluster,
}

impl SeaOrmDatabaseMonitor {
    pub fn new(database: crate::ControlDatabaseCluster) -> Self {
        Self { database }
    }

    pub async fn ping(&self) -> bool {
        // 就绪探针只保障写路径；副本与业务数据源的状态由运行时拓扑端点读取快照。
        matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                crate::connection::ping(self.database.write()),
            )
            .await,
            Ok(Ok(()))
        )
    }

    pub async fn active_connections(&self) -> Option<i64> {
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

    pub async fn topology_health(&self) -> DatabaseTopologyHealth {
        self.database.health().await
    }
}
