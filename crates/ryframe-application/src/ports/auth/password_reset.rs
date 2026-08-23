use chrono::{DateTime, Utc};
use ryframe_kernel::DataScopeContext;

pub const PASSWORD_RESET_STATUS_PENDING: &str = "pending";

#[derive(Clone, Debug)]
pub struct PasswordResetRequestRecord {
    pub id: i64,
    pub tenant_id: String,
    pub target_user_id: i64,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: String,
}

#[derive(Debug)]
pub struct NewPasswordResetRequest {
    pub id: i64,
    pub tenant_id: String,
    pub target_user_id: i64,
    pub requested_by: i64,
    pub reason: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub request_ip: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PasswordResetUserState {
    pub id: i64,
    pub authorization_version: i32,
    pub status: String,
    pub has_super_role: bool,
}

#[async_trait::async_trait]
pub trait PasswordResetTransaction: crate::PersistenceTransaction + Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> ryframe_kernel::AppResult<Option<PasswordResetUserState>>;

    async fn insert_request(
        &self,
        request: NewPasswordResetRequest,
    ) -> ryframe_kernel::AppResult<PasswordResetRequestRecord>;

    async fn lock_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> ryframe_kernel::AppResult<Option<PasswordResetRequestRecord>>;

    async fn lock_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<PasswordResetUserState>>;

    async fn expire_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        evaluated_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn complete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        completed_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        expected: &'a PasswordResetUserState,
        password_hash: String,
        next_status: String,
        updated_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn record_user_mirror_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        authorization_version: i32,
    ) -> ryframe_kernel::AppResult<()>;
}

#[async_trait::async_trait]
pub trait PasswordResetPersistencePort: Send + Sync {
    async fn database_now(&self) -> ryframe_kernel::AppResult<DateTime<Utc>>;

    async fn find_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> ryframe_kernel::AppResult<Option<PasswordResetRequestRecord>>;

    async fn find_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<PasswordResetUserState>>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn PasswordResetTransaction>>;
}
