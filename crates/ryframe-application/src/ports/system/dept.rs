use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};

use crate::PersistenceTransaction;

#[derive(Debug)]
pub struct DeptRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub ancestors: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct DeptTreeRecord {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub sort: i32,
    pub status: String,
    pub children: Vec<DeptTreeRecord>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeptFilter<'a> {
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[async_trait]
pub trait DeptReadPort: Send + Sync {
    async fn find_child_ids(&self, tenant_id: &str, dept_id: i64) -> AppResult<Vec<i64>>;

    async fn find_tree(
        &self,
        tenant_id: &str,
        visible_ids: Option<&[i64]>,
    ) -> AppResult<Vec<DeptTreeRecord>>;

    async fn find_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: DeptFilter<'_>,
        visible_ids: Option<&[i64]>,
    ) -> AppResult<PageResult<DeptRecord>>;

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<DeptRecord>>;
}

#[async_trait]
pub trait DeptWriteTransaction: PersistenceTransaction + Sync {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()>;

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<DeptRecord>>;

    async fn find_descendants_for_update(
        &self,
        tenant_id: &str,
        old_prefix: &str,
    ) -> AppResult<Vec<DeptRecord>>;

    async fn insert(&self, tenant_id: &str, record: DeptRecord) -> AppResult<DeptRecord>;

    async fn update(&self, tenant_id: &str, record: DeptRecord) -> AppResult<DeptRecord>;

    async fn has_child_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool>;

    async fn has_reference_for_update(&self, tenant_id: &str, id: i64) -> AppResult<bool>;

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()>;

    async fn increment_authorization_epoch(&self, tenant_id: &str) -> AppResult<i32>;

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()>;
}

#[async_trait]
pub trait DeptWritePort: Send + Sync {
    async fn begin(&self) -> AppResult<Box<dyn DeptWriteTransaction>>;
}
