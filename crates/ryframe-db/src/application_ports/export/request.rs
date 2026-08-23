use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ConfigFilter, ConfigRepository, ControlDatabaseCluster,
    CreateExportJob, DictTypeFilter, DictTypeRepository, ExportJobRepository, LoginInfoFilter,
    LoginInfoRepository, OperLogFilter, OperLogRepository, PostExportFilter, PostExportRepository,
    RoleFilter, RoleRepository, UserFilter, UserRepository,
};
use ryframe_kernel::{ActorContext, AppError, ExportQuerySnapshot};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob,
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

#[async_trait::async_trait]
impl ExportRequestPersistencePort for DatabaseExportRequestPersistence {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ExportRequestTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabaseExportRequestTransaction { transaction })
            as Box<dyn ExportRequestTransaction>)
    }
}

#[async_trait::async_trait]
impl ExportRequestTransaction for DatabaseExportRequestTransaction {
    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(&self.transaction).await
    }

    async fn find_active<'a>(
        &'a self,
        tenant_id: &'a str,
        requester_id: i64,
        request_fingerprint: &'a str,
    ) -> ryframe_kernel::AppResult<Option<ExportRequesterRecord>> {
        ExportJobRepository
            .find_active_by_fingerprint_for_update(
                &self.transaction,
                tenant_id,
                requester_id,
                request_fingerprint,
            )
            .await
            .map(|record| record.map(super::mapping::requester_record))
    }

    async fn summarize_selection<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: &'a ActorContext,
        selection: &'a ExportSelection,
    ) -> ryframe_kernel::AppResult<ExportQuerySnapshot> {
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
                PostExportRepository
                    .summarize(
                        &self.transaction,
                        tenant_id,
                        &PostExportFilter {
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
    }

    async fn enqueue_job(
        &self,
        command: EnqueueJob,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<i64> {
        BackgroundJobRepository
            .enqueue_in_transaction(
                &self.transaction,
                super::super::jobs::database_enqueue(command),
                now,
            )
            .await
            .map(|result| result.job.id)
    }

    async fn create_export(
        &self,
        command: CreateExportRecord,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ryframe_kernel::AppResult<ExportRequesterRecord> {
        ExportJobRepository
            .create_in_transaction(&self.transaction, database_create(command), now)
            .await
            .map(super::mapping::requester_record)
    }

    async fn commit(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        super::super::audit::commit_current_audit(self.transaction).await
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

pub fn database_create(command: CreateExportRecord) -> CreateExportJob {
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
