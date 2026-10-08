use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    BackgroundJobRepository, ConfigFilter, ConfigRepository, ControlDatabaseCluster,
    CreateExportJob, DictTypeFilter, DictTypeRepository, ExportJobRepository, LoginInfoFilter,
    LoginInfoRepository, OperLogFilter, OperLogRepository, PostExportFilter, PostExportRepository,
    RoleFilter, RoleRepository, UserFilter, UserRepository,
};
use ryframe_kernel::{ActorContext, ExportQuerySnapshot};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    EnqueueJob,
    ports::export::{
        CreateExportRecord, ExportRequestPersistencePort, ExportRequestTransaction,
        ExportRequesterRecord,
    },
    system::operations::ExportSelection,
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
        let transaction = self.database.write().begin().await.db()?;
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
                        &user_filter(filter),
                        &data_scope,
                        selection.selected_ids(),
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
                        selection.selected_ids(),
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
                        selection.selected_ids(),
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
                        selection.selected_ids(),
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
                        selection.selected_ids(),
                    )
                    .await
            }
            ExportSelection::OperLogs(filter) => {
                OperLogRepository
                    .summarize_export(
                        &self.transaction,
                        tenant_id,
                        &oper_filter(filter),
                        &data_scope,
                        selection.selected_ids(),
                    )
                    .await
            }
            ExportSelection::LoginLogs(filter) => {
                LoginInfoRepository
                    .summarize_export(
                        &self.transaction,
                        tenant_id,
                        &login_filter(filter),
                        &data_scope,
                        selection.selected_ids(),
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
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseExportRequestTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            ryframe_application::TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            ryframe_application::TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
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

fn user_filter(
    filter: &ryframe_application::system::operations::UserExportFilter,
) -> UserFilter<'_> {
    UserFilter {
        username: filter.username(),
        phone: filter.phone(),
        status: filter.status(),
        dept_id: filter.dept_id(),
    }
}

fn login_filter(
    filter: &ryframe_application::system::operations::LoginLogExportFilter,
) -> LoginInfoFilter<'_> {
    LoginInfoFilter {
        user_name: filter.user_name(),
        status: filter.status(),
        begin_time: filter.begin_time(),
        end_time: filter.end_time(),
    }
}

fn oper_filter(
    filter: &ryframe_application::system::operations::OperLogExportFilter,
) -> OperLogFilter<'_> {
    OperLogFilter {
        oper_name: filter.oper_name(),
        status: filter.status(),
        begin_time: filter.begin_time(),
        end_time: filter.end_time(),
    }
}
