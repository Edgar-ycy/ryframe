use chrono::{DateTime, Utc};
use ryframe_kernel::DataScopeContext;

use crate::PersistenceFuture;

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

pub trait PasswordResetTransaction: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>>;

    fn insert_request(
        &self,
        request: NewPasswordResetRequest,
    ) -> PersistenceFuture<'_, PasswordResetRequestRecord>;

    fn lock_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetRequestRecord>>;

    fn lock_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>>;

    fn expire_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        evaluated_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn complete_pending<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
        completed_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        expected: &'a PasswordResetUserState,
        password_hash: String,
        next_status: String,
        updated_at: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn record_user_mirror_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        authorization_version: i32,
    ) -> PersistenceFuture<'a, ()>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait PasswordResetPersistencePort: Send + Sync {
    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_request<'a>(
        &'a self,
        tenant_id: &'a str,
        request_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetRequestRecord>>;

    fn find_user_state<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<PasswordResetUserState>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn PasswordResetTransaction>>;
}
