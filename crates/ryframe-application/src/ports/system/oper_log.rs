use chrono::{DateTime, Utc};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait OperLogTransaction: ControlTransaction {
    fn clean<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64>;
}

pub trait OperLogPersistencePort: Send + Sync {
    fn insert<'a>(&'a self, tenant_id: &'a str, record: OperLogRecord)
    -> PersistenceFuture<'a, ()>;

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: OperLogFilter<'a>,
        data_scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<OperLogRecord>>;

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: OperLogFilter<'a>,
        data_scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<OperLogRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn OperLogTransaction>>;
}
