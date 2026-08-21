use chrono::{DateTime, Utc};
use ryframe_kernel::{PageResult, ValidatedPageQuery};

use crate::{ControlTransaction, PersistenceFuture};

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

pub trait DeptReadPort: Send + Sync {
    fn find_child_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn find_tree<'a>(
        &'a self,
        tenant_id: &'a str,
        visible_ids: Option<&'a [i64]>,
    ) -> PersistenceFuture<'a, Vec<DeptTreeRecord>>;

    fn find_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: DeptFilter<'a>,
        visible_ids: Option<&'a [i64]>,
    ) -> PersistenceFuture<'a, PageResult<DeptRecord>>;

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DeptRecord>>;
}

pub trait DeptWriteTransaction: ControlTransaction + Sync {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DeptRecord>>;

    fn find_descendants_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        old_prefix: &'a str,
    ) -> PersistenceFuture<'a, Vec<DeptRecord>>;

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DeptRecord,
    ) -> PersistenceFuture<'a, DeptRecord>;

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DeptRecord,
    ) -> PersistenceFuture<'a, DeptRecord>;

    fn has_child_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn has_reference_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, bool>;

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()>;

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32>;

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()>;
}

pub trait DeptWritePort: Send + Sync {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn DeptWriteTransaction>>;
}
