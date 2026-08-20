use chrono::{DateTime, Utc};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginInfoRecord {
    pub id: i64,
    pub user_name: String,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub login_time: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct LoginInfoFilter<'a> {
    pub user_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

pub trait LoginInfoTransaction: ControlTransaction {
    fn clean<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, u64>;
}

pub trait LoginInfoPersistencePort: Send + Sync {
    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: LoginInfoRecord,
    ) -> PersistenceFuture<'a, ()>;

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: LoginInfoFilter<'a>,
        data_scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, PageResult<LoginInfoRecord>>;

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: LoginInfoFilter<'a>,
        data_scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<LoginInfoRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn LoginInfoTransaction>>;
}
