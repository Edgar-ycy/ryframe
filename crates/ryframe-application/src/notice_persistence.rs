use chrono::{DateTime, Utc};
use ryframe_kernel::{DataScopeContext, PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait NoticeTransaction: ControlTransaction {
    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<NoticeRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: NoticeRecord,
    ) -> PersistenceFuture<'a, NoticeRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: NoticeRecord,
    ) -> PersistenceFuture<'a, NoticeRecord>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;
}

pub trait NoticePersistencePort: Send + Sync {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<NoticeRecord>>;

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: NoticeFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<NoticeRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn NoticeTransaction>>;
}
