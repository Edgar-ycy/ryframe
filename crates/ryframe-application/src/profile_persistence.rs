use chrono::{DateTime, Utc};

use crate::{ControlTransaction, PersistenceFuture};

#[derive(Debug)]
pub struct ProfileRecord {
    pub user_id: i64,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub preferred_locale: Option<String>,
    pub dept_id: Option<i64>,
    pub dept_name: Option<String>,
    pub status: String,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug)]
pub struct ProfileUserState {
    pub password_hash: String,
    pub avatar_file_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileAvatarState {
    Ready,
    Cleanup,
    Unavailable,
}

#[derive(Debug)]
pub struct ProfileAvatarFile {
    pub bucket: String,
    pub state: ProfileAvatarState,
}

pub trait ProfileTransaction: ControlTransaction + Sync {
    fn find_user_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileUserState>>;

    fn update_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> PersistenceFuture<'a, ()>;

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()>;

    fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        password_hash: String,
    ) -> PersistenceFuture<'a, ()>;

    fn increment_user_authorization_version<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Vec<(i64, i32)>>;

    fn database_now(&self) -> PersistenceFuture<'_, DateTime<Utc>>;

    fn find_avatar_file_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileAvatarFile>>;

    fn restore_avatar_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn update_avatar<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, ()>;

    fn count_avatar_references<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> PersistenceFuture<'a, u64>;

    fn mark_avatar_orphan<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: DateTime<Utc>,
        cleanup_after: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait ProfilePersistencePort: Send + Sync {
    fn find_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> PersistenceFuture<'a, Option<ProfileRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ProfileTransaction>>;
}
