use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, OperLogFilter as DatabaseOperLogFilter, OperLogRepository,
    ReadConsistency, Repository, entities::oper_log,
};
use async_trait::async_trait;
use ryframe_kernel::{
    AppResult, DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction},
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

#[async_trait]
impl OperLogPersistencePort for DatabaseOperLogPersistence {
    async fn insert(&self, tenant_id: &str, record: OperLogRecord) -> AppResult<()> {
        OperLogRepository
            .insert(
                self.database.write(),
                tenant_id,
                to_entity(tenant_id, record),
            )
            .await
            .map(|_| ())
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContext,
    ) -> AppResult<PageResult<OperLogRecord>> {
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
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContext,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<OperLogRecord>> {
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
    }

    async fn begin(&self) -> AppResult<Box<dyn OperLogTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseOperLogTransaction { transaction }) as Box<dyn OperLogTransaction>)
    }
}

#[async_trait]
impl OperLogTransaction for DatabaseOperLogTransaction {
    async fn clean(&self, tenant_id: &str) -> AppResult<u64> {
        OperLogRepository
            .clean_all_in_transaction(&self.transaction, tenant_id)
            .await
    }
}

#[async_trait]
impl PersistenceTransaction for DatabaseOperLogTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
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
