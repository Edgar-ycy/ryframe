use super::transaction::DatabasePortTransaction;
use crate::{ControlDatabaseCluster, DbResultExt, repositories::backup_repo};
use chrono::{DateTime, Utc};
use ryframe_application::ports::backup::*;
use ryframe_kernel::AppResult;
use sea_orm::TransactionTrait;
use std::sync::Arc;

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn BackupRepository> {
    Arc::new(DatabaseBackups { database })
}

struct DatabaseBackups {
    database: ControlDatabaseCluster,
}
struct BackupSqlTransaction {
    transaction: DatabasePortTransaction,
}

#[async_trait::async_trait]
impl BackupRepository for DatabaseBackups {
    async fn required_resources(&self) -> AppResult<Vec<String>> {
        backup_repo::required_resources(self.database.write()).await
    }
    async fn database_now(&self) -> AppResult<DateTime<Utc>> {
        crate::repositories::database_utc_now(self.database.write()).await
    }

    async fn begin(&self) -> AppResult<Box<dyn BackupTransaction>> {
        Ok(Box::new(BackupSqlTransaction {
            transaction: self.database.write().begin().await.db()?.into(),
        }))
    }

    async fn backup(&self, id: &str) -> AppResult<Option<BackupRecord>> {
        backup_repo::backup(self.database.write(), id, false).await
    }

    async fn restore(&self, id: &str) -> AppResult<Option<RestoreRecord>> {
        backup_repo::restore(self.database.write(), id, false).await
    }

    async fn health(
        &self,
        scope_id: &str,
        resources: &[String],
        now: DateTime<Utc>,
    ) -> AppResult<BackupHealth> {
        backup_repo::backup_health(self.database.write(), scope_id, resources, now).await
    }
}

#[async_trait::async_trait]
impl BackupTransaction for BackupSqlTransaction {
    async fn save_backup(&self, record: &BackupRecord) -> AppResult<BackupRecord> {
        backup_repo::save_backup(&self.transaction, record).await
    }

    async fn create_restore(&self, record: &RestoreRecord) -> AppResult<RestoreRecord> {
        backup_repo::create_restore(&self.transaction, record).await
    }

    async fn advance_restore(
        &self,
        expected: &RestoreRecord,
        next: &RestoreRecord,
    ) -> AppResult<RestoreRecord> {
        backup_repo::advance_restore(&self.transaction, expected, next).await
    }

    async fn commit(self: Box<Self>) -> AppResult<()> {
        self.transaction.commit().await.db()
    }
}
