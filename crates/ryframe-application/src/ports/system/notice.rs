use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, DataScopeContext, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoticeRecord {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub notice_type: Option<String>,
    pub status: String,
    pub created_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct NoticeFilter<'a> {
    pub title: Option<&'a str>,
    pub notice_type: Option<&'a str>,
    pub status: Option<&'a str>,
    pub data_scope: &'a DataScopeContext,
}

#[async_trait]
pub trait NoticeTransaction: PersistenceTransaction {
    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<NoticeRecord>>;

    async fn insert(&self, tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord>;

    async fn update(&self, tenant_id: &str, record: NoticeRecord) -> AppResult<NoticeRecord>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;
}

#[async_trait]
pub trait NoticePersistencePort: Send + Sync {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<NoticeRecord>>;

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: NoticeFilter<'_>,
    ) -> AppResult<PageResult<NoticeRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn NoticeTransaction>>;
}
