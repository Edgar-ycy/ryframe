use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

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

#[async_trait]
pub trait PostTransaction: PersistenceTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<PostRecord>>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<PostRecord>>;

    async fn insert(&self, tenant_id: &str, record: PostRecord) -> AppResult<PostRecord>;

    async fn update(&self, tenant_id: &str, record: PostRecord) -> AppResult<PostRecord>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait PostPersistencePort: Send + Sync {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<PostRecord>>;

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: PostFilter<'_>,
    ) -> AppResult<PageResult<PostRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: PostFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostRecord>>;

    async fn begin(&self) -> AppResult<Box<dyn PostTransaction>>;
}
