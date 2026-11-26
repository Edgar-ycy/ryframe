use async_trait::async_trait;
use ryframe_kernel::{
    AppResult, DataScopeContextRef, ExportCursorWindow, PageResult, ValidatedPageQuery,
};
use time::OffsetDateTime;

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
    pub login_time: OffsetDateTime,
}

#[derive(Clone, Copy, Debug)]
pub struct LoginInfoFilter<'a> {
    pub user_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<OffsetDateTime>,
    pub end_time: Option<OffsetDateTime>,
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
        data_scope: &DataScopeContextRef<'_>,
    ) -> AppResult<PageResult<LoginInfoRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: LoginInfoFilter<'_>,
        data_scope: &DataScopeContextRef<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<LoginInfoRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn LoginInfoTransaction>>;
}
