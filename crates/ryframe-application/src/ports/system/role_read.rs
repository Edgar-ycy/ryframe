use chrono::{DateTime, Utc};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};

use crate::PersistenceFuture;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub is_super: i8,
    pub data_scope: String,
    pub status: String,
    pub sort: i32,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoleFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

pub trait RoleReadPort: Send + Sync {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<RoleRecord>>;

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: RoleFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<RoleRecord>>;

    fn find_options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        include_super: bool,
        limit: u64,
    ) -> PersistenceFuture<'a, Vec<RoleRecord>>;

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: RoleFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<RoleRecord>>;

    fn find_super_role<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<RoleRecord>>;

    fn find_role_dept_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>>;

    fn find_permission_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Option<Vec<String>>>;
}
