use chrono::{DateTime, Utc};
use ryframe_kernel::DataScopeContext;

pub const USER_STATUS_DISABLED: &str = "0";
pub const USER_STATUS_NORMAL: &str = "1";
pub const USER_STATUS_PENDING_ACTIVATION: &str = "pending_activation";
pub const USER_STATUS_MUST_RESET_PASSWORD: &str = "must_reset_password";

#[derive(Clone, Debug)]
pub struct UserWriteRecord {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub status: String,
    pub dept_id: Option<i64>,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct NewUserRecord {
    pub id: i64,
    pub tenant_id: String,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub dept_id: Option<i64>,
}

#[derive(Debug)]
pub struct UpdateUserRecord {
    pub id: i64,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub dept_id: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
pub struct UserAssignmentRole {
    pub status_normal: bool,
    pub is_super: bool,
}

#[derive(Debug)]
pub struct UserAssignmentState {
    pub department_exists: bool,
    pub roles: Vec<UserAssignmentRole>,
}

#[derive(Debug)]
pub struct ManageableUserState {
    pub user: UserWriteRecord,
    pub has_super_role: bool,
}

#[async_trait::async_trait]
pub trait UserWriteTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<UserAssignmentState>;

    async fn ensure_user_quota<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<Option<ManageableUserState>>;

    async fn insert_user(&self, user: NewUserRecord) -> ryframe_kernel::AppResult<UserWriteRecord>;

    async fn update_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user: UpdateUserRecord,
    ) -> ryframe_kernel::AppResult<UserWriteRecord>;

    async fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        status: String,
    ) -> ryframe_kernel::AppResult<()>;

    async fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<()>;

    async fn increment_authorization_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>>;

    async fn delete_users<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<u64>;
}

#[async_trait::async_trait]
pub trait UserWritePersistencePort: Send + Sync {
    async fn username_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<UserAssignmentState>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn UserWriteTransaction>>;
}
