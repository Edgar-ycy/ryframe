use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};

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

#[async_trait]
pub trait RoleReadPort: Send + Sync {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<RoleRecord>>;

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: RoleFilter<'_>,
    ) -> AppResult<PageResult<RoleRecord>>;

    async fn find_options(
        &self,
        tenant_id: &str,
        query: Option<&str>,
        include_super: bool,
        limit: u64,
    ) -> AppResult<Vec<RoleRecord>>;

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: RoleFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<RoleRecord>>;

    async fn find_super_role(&self, tenant_id: &str) -> AppResult<Option<RoleRecord>>;

    async fn find_role_dept_ids(&self, tenant_id: &str, role_id: i64) -> AppResult<Vec<i64>>;

    async fn find_permission_codes(
        &self,
        tenant_id: &str,
        role_id: i64,
    ) -> AppResult<Option<Vec<String>>>;
}
