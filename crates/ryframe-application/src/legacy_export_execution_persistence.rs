use std::sync::Arc;

use ryframe_db::{
    BackgroundJobRepository, ControlDatabaseCluster, ExportJobRepository,
    ExportStartDisposition as DatabaseStartDisposition,
    entities::{background_job, export_job},
};
use ryframe_kernel::AppError;
use sea_orm::{DatabaseTransaction, TransactionTrait};

use crate::{
    ExportBackgroundLease, ExportExecutionPersistencePort, ExportExecutionRecord,
    ExportExecutionState, ExportExecutionTransaction, ExportStartDecision, PersistenceFuture,
};

struct LegacyExportExecutionPersistence {
    database: ControlDatabaseCluster,
}

struct LegacyExportExecutionTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportExecutionPersistencePort> {
    Arc::new(LegacyExportExecutionPersistence { database })
}

impl ExportExecutionPersistencePort for LegacyExportExecutionPersistence {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move {
            BackgroundJobRepository
                .database_utc_now(self.database.write())
                .await
        })
    }

    fn find_by_background_job(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportExecutionRecord>> {
        Box::pin(async move {
            ExportJobRepository
                .find_by_background_job_id(self.database.write(), background_job_id)
                .await
                .map(|record| record.map(map_execution_record))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportExecutionTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyExportExecutionTransaction { transaction })
                as Box<dyn ExportExecutionTransaction>)
        })
    }

    fn update_exported_rows(
        &self,
        export_id: i64,
        exported_rows: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, bool> {
        Box::pin(async move {
            ExportJobRepository
                .update_exported_rows(self.database.write(), export_id, exported_rows, now)
                .await
        })
    }

    fn find_background_lease(
        &self,
        background_job_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportBackgroundLease>> {
        Box::pin(async move {
            BackgroundJobRepository
                .find_by_id(self.database.write(), background_job_id)
                .await
                .map(|record| record.map(map_background_lease))
        })
    }

    fn find_export_state(
        &self,
        export_id: i64,
    ) -> PersistenceFuture<'_, Option<ExportExecutionState>> {
        Box::pin(async move {
            ExportJobRepository
                .find_by_id(self.database.write(), export_id)
                .await
                .map(|record| {
                    record.map(|record| ExportExecutionState {
                        status: record.status,
                        delete_pending_at: record.delete_pending_at,
                    })
                })
        })
    }
}

impl ExportExecutionTransaction for LegacyExportExecutionTransaction {
    fn try_start<'a>(
        &'a self,
        export_id: i64,
        tenant_id: &'a str,
        maximum_running: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ExportStartDecision> {
        Box::pin(async move {
            ExportJobRepository
                .try_mark_running_in_transaction(
                    &self.transaction,
                    export_id,
                    tenant_id,
                    maximum_running,
                    now,
                )
                .await
                .map(map_start_decision)
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn map_execution_record(record: export_job::Model) -> ExportExecutionRecord {
    ExportExecutionRecord {
        id: record.id,
        tenant_id: record.tenant_id,
        requester_id: record.requester_id,
        resource: record.resource,
        request_params: record.request_params,
        request_version: record.request_version,
        permission_code: record.permission_code,
        authorization_fingerprint: record.authorization_fingerprint,
        snapshot_at: record.snapshot_at,
        upper_id: record.upper_id,
        matched_rows: record.matched_rows,
        status: record.status,
    }
}

fn map_background_lease(record: background_job::Model) -> ExportBackgroundLease {
    ExportBackgroundLease {
        status: record.status,
        lease_owner: record.lease_owner,
        lease_until: record.lease_until,
    }
}

fn map_start_decision(value: DatabaseStartDisposition) -> ExportStartDecision {
    match value {
        DatabaseStartDisposition::Started => ExportStartDecision::Started,
        DatabaseStartDisposition::AlreadyRunning => ExportStartDecision::AlreadyRunning,
        DatabaseStartDisposition::ConcurrencyLimited => ExportStartDecision::ConcurrencyLimited,
        DatabaseStartDisposition::NotRunnable => ExportStartDecision::NotRunnable,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_start_disposition_maps_to_application_state() {
        assert_eq!(
            map_start_decision(DatabaseStartDisposition::Started),
            ExportStartDecision::Started
        );
        assert_eq!(
            map_start_decision(DatabaseStartDisposition::AlreadyRunning),
            ExportStartDecision::AlreadyRunning
        );
        assert_eq!(
            map_start_decision(DatabaseStartDisposition::ConcurrencyLimited),
            ExportStartDecision::ConcurrencyLimited
        );
        assert_eq!(
            map_start_decision(DatabaseStartDisposition::NotRunnable),
            ExportStartDecision::NotRunnable
        );
    }
}
