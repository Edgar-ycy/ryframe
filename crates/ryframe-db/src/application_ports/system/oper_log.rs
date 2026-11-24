use std::sync::Arc;

use async_trait::async_trait;
use ryframe_kernel::{
    AppResult, DataScopeContextRef, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use time::{PrimitiveDateTime, UtcOffset};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction},
};

use crate::{
    ControlDatabaseCluster, DbResultExt, OperLogFilter as DatabaseOperLogFilter, OperLogRepository,
    ReadConsistency, Repository, entities::oper_log,
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
        .insert_event_in_transaction(transaction, tenant_id, to_entity(tenant_id, record)?)
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
                to_entity(tenant_id, record)?,
            )
            .await
            .map(|_| ())
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContextRef<'_>,
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
        let records = result.records.into_iter().map(to_record).collect();
        Ok(PageResult::new(records, result.total, &page))
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContextRef<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<OperLogRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        Ok(OperLogRepository
            .find_for_export_after_id(
                &database,
                tenant_id,
                &to_database_filter(filter),
                data_scope,
                window,
            )
            .await?
            .into_iter()
            .map(to_record)
            .collect())
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
        oper_time: model.oper_time.assume_utc(),
        cost_time: model.cost_time,
    }
}

fn to_entity(tenant_id: &str, record: OperLogRecord) -> AppResult<oper_log::Model> {
    let oper_time = record
        .oper_time
        .checked_to_offset(UtcOffset::UTC)
        .ok_or("时间超出 UTC 表示范围")
        .db_context("写入操作日志 UTC 时间")?;
    Ok(oper_log::Model {
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
        oper_time: PrimitiveDateTime::new(oper_time.date(), oper_time.time()),
        cost_time: record.cost_time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryframe_application::system::operations::OperLogVo;
    use ryframe_kernel::AppError;
    use time::macros::datetime;

    #[test]
    fn operation_time_preserves_sql_precision_and_utc_view() {
        let entity = oper_log::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            event_id: None,
            request_id: None,
            title: "测试操作".into(),
            business_type: "UPDATE".into(),
            method: "update".into(),
            request_method: "PUT".into(),
            oper_name: "tester".into(),
            oper_url: "/fixture".into(),
            oper_ip: "127.0.0.1".into(),
            oper_location: None,
            oper_param: None,
            json_result: None,
            status: "1".into(),
            error_msg: None,
            oper_time: datetime!(1969-12-31 23:59:59.123456),
            cost_time: 10,
        };
        let mut record = to_record(entity.clone());
        assert_eq!(record.oper_time.unix_timestamp(), -1);
        assert_eq!(record.oper_time.nanosecond(), 123_456_000);
        assert_eq!(record.oper_time.offset(), UtcOffset::UTC);

        record.oper_time = record
            .oper_time
            .to_offset(UtcOffset::from_hms(8, 0, 0).unwrap());
        let view = OperLogVo::try_from(record.clone()).unwrap();
        assert_eq!(view.oper_time, "1969-12-31 23:59:59");
        assert_eq!(to_entity("tenant-a", record.clone()).unwrap(), entity);

        let encoded = serde_json::to_value(&entity).unwrap();
        assert_eq!(encoded["oper_time"], "1969-12-31T23:59:59.123456Z");
        assert_eq!(
            serde_json::from_value::<oper_log::Model>(encoded.clone()).unwrap(),
            entity
        );

        for unrepresentable in ["+10000-01-01T00:00:00Z", "1969-12-31T23:59:60.123456Z"] {
            let mut invalid = encoded.clone();
            invalid["oper_time"] = serde_json::json!(unrepresentable);
            assert!(serde_json::from_value::<oper_log::Model>(invalid).is_err());
        }

        record.oper_time =
            PrimitiveDateTime::MAX.assume_offset(UtcOffset::from_hms(-1, 0, 0).unwrap());
        assert!(matches!(
            to_entity("tenant-a", record),
            Err(AppError::Database(_))
        ));
    }
}
