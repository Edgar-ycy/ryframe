use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ConfigFilter, ConfigRepository, ControlDatabaseCluster,
    CreateExportJob, DictTypeFilter, DictTypeRepository, ExportJobRepository, LoginInfoFilter,
    LoginInfoRepository, OperLogFilter, OperLogRepository, PostFilter, PostRepository, RoleFilter,
    RoleRepository, UserFilter, UserRepository,
};
use ryframe_kernel::{ActorContext, AppError, ExportQuerySnapshot};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob, PersistenceFuture,
    ports::export::{
        CreateExportRecord, ExportRequestPersistencePort, ExportRequestTransaction,
        ExportRequesterRecord,
    },
    system::ExportSelection,
};

struct DatabaseExportRequestPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseExportRequestTransaction {
    transaction: DatabaseTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ExportRequestPersistencePort> {
    Arc::new(DatabaseExportRequestPersistence { database })
}

impl ExportRequestPersistencePort for DatabaseExportRequestPersistence {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ExportRequestTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseExportRequestTransaction { transaction })
                as Box<dyn ExportRequestTransaction>)
        })
    }
}

impl ExportRequestTransaction for DatabaseExportRequestTransaction {
    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move { crate::repositories::database_utc_now(&self.transaction).await })
    }

    fn find_active<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        request_fingerprint: &'a str,
    ) -> PersistenceFuture<'a, Option<ExportRequesterRecord>> {
        Box::pin(async move {
            ExportJobRepository
                .find_active_by_fingerprint_for_update(
                    &self.transaction,
                    tenant_id,
                    requester_id,
                    request_fingerprint,
                )
                .await
                .map(|record| record.map(super::mapping::requester_record))
        })
    }

    fn summarize_selection<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: &'a ActorContext,
        selection: &'a ExportSelection,
    ) -> PersistenceFuture<'a, ExportQuerySnapshot> {
        Box::pin(async move {
            let data_scope = actor.data_scope_context();
            match selection {
                ExportSelection::Users(filter) => {
                    UserRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &UserFilter {
                                username: filter.username(),
                                phone: filter.phone(),
                                status: filter.status(),
                                dept_id: filter.dept_id(),
                            },
                            &data_scope,
                        )
                        .await
                }
                ExportSelection::Roles(filter) => {
                    RoleRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &RoleFilter {
                                name: filter.name(),
                                code: filter.code(),
                                status: filter.status(),
                            },
                        )
                        .await
                }
                ExportSelection::Posts(filter) => {
                    PostRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &PostFilter {
                                name: filter.name(),
                                code: filter.code(),
                                status: filter.status(),
                            },
                        )
                        .await
                }
                ExportSelection::Configs(filter) => {
                    ConfigRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &ConfigFilter {
                                name: filter.name(),
                                key: filter.key(),
                            },
                        )
                        .await
                }
                ExportSelection::DictTypes(filter) => {
                    DictTypeRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &DictTypeFilter {
                                name: filter.name(),
                                code: filter.code(),
                                status: filter.status(),
                            },
                        )
                        .await
                }
                ExportSelection::OperLogs(filter) => {
                    OperLogRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &OperLogFilter {
                                oper_name: filter.oper_name(),
                                status: filter.status(),
                                begin_time: filter.begin_time(),
                                end_time: filter.end_time(),
                            },
                            &data_scope,
                        )
                        .await
                }
                ExportSelection::LoginLogs(filter) => {
                    LoginInfoRepository
                        .summarize_export(
                            &self.transaction,
                            tenant_id,
                            &LoginInfoFilter {
                                user_name: filter.user_name(),
                                status: filter.status(),
                                begin_time: filter.begin_time(),
                                end_time: filter.end_time(),
                            },
                            &data_scope,
                        )
                        .await
                }
            }
        })
    }

    fn enqueue_job(
        &self,
        command: EnqueueJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, i64> {
        Box::pin(async move {
            BackgroundJobRepository
                .enqueue_in_transaction(
                    &self.transaction,
                    super::super::jobs::database_enqueue(command),
                    now,
                )
                .await
                .map(|result| result.job.id)
        })
    }

    fn create_export(
        &self,
        command: CreateExportRecord,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, ExportRequesterRecord> {
        Box::pin(async move {
            ExportJobRepository
                .create_in_transaction(&self.transaction, database_create(command), now)
                .await
                .map(super::mapping::requester_record)
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { super::super::audit::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn database_create(command: CreateExportRecord) -> CreateExportJob {
    CreateExportJob {
        tenant_id: command.tenant_id,
        requester_id: command.requester_id,
        resource: command.resource,
        background_job_id: command.background_job_id,
        request_params: command.request_params,
        request_version: command.request_version,
        permission_code: command.permission_code,
        authorization_fingerprint: command.authorization_fingerprint,
        request_fingerprint: command.request_fingerprint,
        snapshot_at: command.snapshot_at,
        upper_id: command.upper_id,
        matched_rows: command.matched_rows,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn create_mapping_moves_every_snapshot_field() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let command = database_create(CreateExportRecord {
            tenant_id: "tenant-a".into(),
            requester_id: 7,
            resource: "users".into(),
            background_job_id: 9,
            request_params: serde_json::json!({"request_version": 2}),
            request_version: 2,
            permission_code: "system:user:export".into(),
            authorization_fingerprint: "authorization".into(),
            request_fingerprint: "request".into(),
            snapshot_at: now,
            upper_id: 99,
            matched_rows: 8,
        });

        assert_eq!(command.tenant_id, "tenant-a");
        assert_eq!(command.requester_id, 7);
        assert_eq!(command.resource, "users");
        assert_eq!(command.background_job_id, 9);
        assert_eq!(command.request_version, 2);
        assert_eq!(command.permission_code, "system:user:export");
        assert_eq!(command.authorization_fingerprint, "authorization");
        assert_eq!(command.request_fingerprint, "request");
        assert_eq!(command.snapshot_at, now);
        assert_eq!(command.upper_id, 99);
        assert_eq!(command.matched_rows, 8);
    }
}
