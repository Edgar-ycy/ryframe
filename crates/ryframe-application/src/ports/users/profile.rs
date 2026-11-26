use time::OffsetDateTime;

use crate::PersistenceTransaction;

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
    pub login_date: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
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

#[async_trait::async_trait]
pub trait ProfileTransaction: PersistenceTransaction + Sync {
    async fn find_user_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileUserState>>;

    async fn update_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        nickname: String,
        email: String,
        phone: String,
        preferred_locale: Option<String>,
    ) -> ryframe_kernel::AppResult<()>;

    async fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> ryframe_kernel::AppResult<()>;

    async fn update_password<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        password_hash: String,
    ) -> ryframe_kernel::AppResult<()>;

    /// 自助改密同样撤销此前签发的待处理重置请求。
    async fn expire_pending_requests<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<()>;

    async fn increment_user_authorization_version<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>>;

    async fn database_now(&self) -> ryframe_kernel::AppResult<OffsetDateTime>;

    async fn find_avatar_file_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileAvatarFile>>;

    async fn restore_avatar_file<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn update_avatar<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        now: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<()>;

    async fn count_avatar_references<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn mark_avatar_orphan<'a>(
        &'a self,
        tenant_id: &'a str,
        file_id: i64,
        now: OffsetDateTime,
        cleanup_after: OffsetDateTime,
    ) -> ryframe_kernel::AppResult<bool>;
}

#[async_trait::async_trait]
pub trait ProfilePersistencePort: Send + Sync {
    async fn find_profile<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProfileRecord>>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ProfileTransaction>>;
}
