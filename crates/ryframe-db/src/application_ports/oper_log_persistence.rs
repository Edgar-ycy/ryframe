use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, OperLogFilter as DatabaseOperLogFilter, OperLogRepository,
    ReadConsistency, Repository, entities::oper_log,
};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    ControlTransaction, OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction,
    PersistenceFuture,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn OperLogPersistencePort> {
    Arc::new(DatabaseOperLogPersistence { database })
}

pub(crate) async fn insert_event_in_transaction(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    record: OperLogRecord,
) -> ryframe_kernel::AppResult<bool> {
    OperLogRepository
        .insert_event_in_transaction(transaction, tenant_id, to_entity(tenant_id, record))
        .await
}

struct DatabaseOperLogPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseOperLogTransaction {
    transaction: DatabaseTransaction,
}

impl OperLogPersistencePort for DatabaseOperLogPersistence {
    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: OperLogRecord,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            OperLogRepository
                .insert(
                    self.database.write(),
                    tenant_id,
                    to_entity(tenant_id, record),
                )
                .await
                .map(|_| ())
        })
    }

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: OperLogFilter<'a>,
        data_scope: &'a ryframe_kernel::DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<OperLogRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            let result = OperLogRepository
                .find_by_page_filtered(
                    &database,
                    tenant_id,
                    &page,
                    to_database_filter(filter),
                    data_scope,
                )
                .await?;
            Ok(PageResult::new(
                result.records.into_iter().map(to_record).collect(),
                result.total,
                &page,
            ))
        })
    }

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: OperLogFilter<'a>,
        data_scope: &'a ryframe_kernel::DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<OperLogRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            OperLogRepository
                .find_for_export_after_id(
                    &database,
                    tenant_id,
                    &to_database_filter(filter),
                    data_scope,
                    window,
                )
                .await
                .map(|records| records.into_iter().map(to_record).collect())
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn OperLogTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseOperLogTransaction { transaction }) as Box<dyn OperLogTransaction>)
        })
    }
}

impl OperLogTransaction for DatabaseOperLogTransaction {
    fn clean<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            OperLogRepository
                .clean_all_in_transaction(&self.transaction, tenant_id)
                .await
        })
    }
}

impl ControlTransaction for DatabaseOperLogTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(
            async move { super::audit_persistence::commit_current_audit(self.transaction).await },
        )
    }
}

fn to_database_filter(filter: OperLogFilter<'_>) -> DatabaseOperLogFilter<'_> {
    DatabaseOperLogFilter {
        oper_name: filter.oper_name,
        status: filter.status,
        begin_time: filter.begin_time,
        end_time: filter.end_time,
    }
}

fn to_record(model: oper_log::Model) -> OperLogRecord {
    OperLogRecord {
        id: model.id,
        event_id: model.event_id,
        request_id: model.request_id,
        title: model.title,
        business_type: model.business_type,
        method: model.method,
        request_method: model.request_method,
        oper_name: model.oper_name,
        oper_url: model.oper_url,
        oper_ip: model.oper_ip,
        oper_location: model.oper_location,
        oper_param: model.oper_param,
        json_result: model.json_result,
        status: model.status,
        error_message: model.error_msg,
        oper_time: model.oper_time,
        cost_time: model.cost_time,
    }
}

fn to_entity(tenant_id: &str, record: OperLogRecord) -> oper_log::Model {
    oper_log::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        event_id: record.event_id,
        request_id: record.request_id,
        title: record.title,
        business_type: record.business_type,
        method: record.method,
        request_method: record.request_method,
        oper_name: record.oper_name,
        oper_url: record.oper_url,
        oper_ip: record.oper_ip,
        oper_location: record.oper_location,
        oper_param: record.oper_param,
        json_result: record.json_result,
        status: record.status,
        error_msg: record.error_message,
        oper_time: record.oper_time,
        cost_time: record.cost_time,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
