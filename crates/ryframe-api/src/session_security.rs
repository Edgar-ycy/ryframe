use ryframe_kernel::AppResult;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRevocation {
    Revoked,
    AlreadyRevoked,
    NotFoundOrForeign,
}

/// 访问令牌主动撤销端口。
#[async_trait::async_trait]
pub trait AccessRevocationStore: Send + Sync {
    async fn is_revoked(&self, jti: &str) -> AppResult<bool>;
    async fn revoke(&self, jti: &str, ttl_seconds: u64) -> AppResult<()>;
}

/// HTTP 认证流程所需的刷新会话控制端口。
#[async_trait::async_trait]
pub trait RefreshSessionControl: Send + Sync {
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
    ) -> AppResult<SessionRevocation>;

    async fn session_sids_for_user(&self, tenant_id: &str, user_id: i64) -> AppResult<Vec<String>>;

    async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidate_sids: &[String],
    ) -> AppResult<u64>;
}
