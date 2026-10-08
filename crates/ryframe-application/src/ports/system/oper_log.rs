use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{
    AppResult, DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery,
};

use crate::PersistenceTransaction;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperLogRecord {
    pub id: i64,
    pub event_id: Option<String>,
    pub request_id: Option<String>,
    pub title: String,
    pub business_type: String,
    pub method: String,
    pub request_method: String,
    pub oper_name: String,
    pub oper_url: String,
    pub oper_ip: String,
    pub oper_location: Option<String>,
    pub oper_param: Option<String>,
    pub json_result: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub oper_time: DateTime<Utc>,
    pub cost_time: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct OperLogFilter<'a> {
    pub oper_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait OperLogTransaction: PersistenceTransaction {
    async fn clean(&self, tenant_id: &str) -> AppResult<u64>;
}

#[async_trait]
pub trait OperLogPersistencePort: Send + Sync {
    async fn insert(&self, tenant_id: &str, record: OperLogRecord) -> AppResult<()>;

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContext,
    ) -> AppResult<PageResult<OperLogRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: OperLogFilter<'_>,
        data_scope: &DataScopeContext,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<OperLogRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn OperLogTransaction>>;
}
