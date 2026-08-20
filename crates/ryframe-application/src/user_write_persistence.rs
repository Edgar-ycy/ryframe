use chrono::{DateTime, Utc};
use ryframe_kernel::DataScopeContext;

use crate::PersistenceFuture;

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

pub trait UserWriteTransaction: Send + Sync {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, UserAssignmentState>;

    fn ensure_user_quota<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn lock_manageable_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        scope: &'a DataScopeContext,
    ) -> PersistenceFuture<'a, Option<ManageableUserState>>;

    fn insert_user(&self, user: NewUserRecord) -> PersistenceFuture<'_, UserWriteRecord>;

    fn update_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user: UpdateUserRecord,
    ) -> PersistenceFuture<'a, UserWriteRecord>;

    fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        status: String,
    ) -> PersistenceFuture<'a, ()>;

    fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()>;

    fn increment_authorization_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, i32)>>;

    fn delete_users<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, u64>;

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait UserWritePersistencePort: Send + Sync {
    fn username_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> PersistenceFuture<'a, bool>;

    fn assignment_state<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: Option<i64>,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, UserAssignmentState>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn UserWriteTransaction>>;
}
