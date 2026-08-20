use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PostFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

pub type PostPersistenceFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

pub trait PostTransaction: Send {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PostPersistenceFuture<'a, ()>;

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PostPersistenceFuture<'a, Option<PostRecord>>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PostPersistenceFuture<'a, Option<PostRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PostRecord,
    ) -> PostPersistenceFuture<'a, PostRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PostRecord,
    ) -> PostPersistenceFuture<'a, PostRecord>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PostPersistenceFuture<'a, ()>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PostPersistenceFuture<'a, ()>;

    fn commit(self: Box<Self>) -> PostPersistenceFuture<'static, ()>;
}

pub trait PostPersistencePort: Send + Sync {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PostPersistenceFuture<'a, Option<PostRecord>>;

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: PostFilter<'a>,
    ) -> PostPersistenceFuture<'a, PageResult<PostRecord>>;

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: PostFilter<'a>,
        window: ExportCursorWindow,
    ) -> PostPersistenceFuture<'a, Vec<PostRecord>>;

    fn begin(&self) -> PostPersistenceFuture<'_, Box<dyn PostTransaction>>;
}
