use chrono::{DateTime, Utc};
use ryframe_kernel::{DataScopeContext, ExportCursorWindow, PageResult, ValidatedPageQuery};

pub const USER_QUERY_STATUS_NORMAL: &str = "1";

#[derive(Clone, Debug)]
pub struct UserQueryRecord {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<i64>,
    pub dept_name: Option<String>,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl UserQueryRecord {
    pub fn is_enabled(&self) -> bool {
        self.status == USER_QUERY_STATUS_NORMAL
    }
}

#[derive(Debug)]
pub struct UserQueryRoleRecord {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub is_super: i8,
}

#[derive(Debug)]
pub struct UserQueryDetailRecord {
    pub user: UserQueryRecord,
    pub roles: Vec<UserQueryRoleRecord>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UserQueryFilter<'a> {
    pub username: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub status: Option<&'a str>,
    pub dept_id: Option<i64>,
}

#[async_trait::async_trait]
pub trait UserQueryReadPort: Send + Sync {
    async fn export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
        window: ExportCursorWindow,
    ) -> ryframe_kernel::AppResult<Vec<UserQueryRecord>>;

    async fn page<'a>(
        &'a self,
        tenant_id: &'a str,
        query: ValidatedPageQuery,
        filter: UserQueryFilter<'a>,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<PageResult<UserQueryRecord>>;

    async fn options<'a>(
        &'a self,
        tenant_id: &'a str,
        query: Option<&'a str>,
        scope: &'a DataScopeContext,
        limit: u64,
    ) -> ryframe_kernel::AppResult<Vec<UserQueryRecord>>;

    async fn detail<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<Option<UserQueryDetailRecord>>;

    async fn is_accessible<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn is_super_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<bool>;
}
