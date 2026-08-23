use ryframe_kernel::AppResult;

#[derive(Debug, Clone)]
pub struct RefreshSessionFamily {
    pub sid: String,
    pub tenant_id: String,
    pub user_id: i64,
    pub current_jti: String,
    pub previous_jti: Option<String>,
    pub last_attempt_id: Option<String>,
    pub rotated_at: i64,
    pub absolute_exp: i64,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSessionIdentity {
    pub tenant_id: String,
    pub user_id: i64,
    pub absolute_exp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshSessionRevocation {
    Revoked,
    AlreadyRevoked,
    NotFoundOrForeign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshSessionRotation {
    Rotated { current_jti: String, issued_at: i64 },
    Recovered { current_jti: String, issued_at: i64 },
    Concurrent,
    Replayed,
    MissingOrRevoked,
}

/// 刷新令牌族的权威状态端口。
#[async_trait::async_trait]
pub trait RefreshSessionPort: Send + Sync {
    async fn register(&self, family: RefreshSessionFamily) -> AppResult<()>;

    async fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> AppResult<RefreshSessionRotation>;

    async fn identity(&self, sid: &str) -> AppResult<Option<RefreshSessionIdentity>>;

    async fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<bool>;

    async fn revoke(&self, sid: &str) -> AppResult<bool>;

    async fn revoke_for_tenant(&self, tenant_id: &str, sid: &str) -> AppResult<bool>;

    async fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
    ) -> AppResult<RefreshSessionRevocation>;

    async fn session_sids_for_user(&self, tenant_id: &str, user_id: i64) -> AppResult<Vec<String>>;

    async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidate_sids: &[String],
    ) -> AppResult<u64>;
}
