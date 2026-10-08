use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{
    AppResult, DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery,
};

use crate::PersistenceTransaction;

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

#[async_trait]
pub trait LoginInfoTransaction: PersistenceTransaction {
    async fn clean(&self, tenant_id: &str) -> AppResult<u64>;
}

#[async_trait]
pub trait LoginInfoPersistencePort: Send + Sync {
    async fn insert(&self, tenant_id: &str, record: LoginInfoRecord) -> AppResult<()>;

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContext,
    ) -> AppResult<PageResult<LoginInfoRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContext,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<LoginInfoRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn LoginInfoTransaction>>;
}
