use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type SessionSecurityFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRevocation {
    Revoked,
    AlreadyRevoked,
    NotFoundOrForeign,
}

/// 访问令牌主动撤销端口。
pub trait AccessRevocationStore: Send + Sync {
    fn is_revoked<'a>(&'a self, jti: &'a str) -> SessionSecurityFuture<'a, bool>;
    fn revoke<'a>(&'a self, jti: &'a str, ttl_seconds: u64) -> SessionSecurityFuture<'a, ()>;
}

/// HTTP 认证流程所需的刷新会话控制端口。
pub trait RefreshSessionControl: Send + Sync {
    fn is_active_for_identity<'a>(
        &'a self,
        sid: &'a str,
        tenant_id: &'a str,
        user_id: i64,
    ) -> SessionSecurityFuture<'a, bool>;

    fn revoke<'a>(&'a self, sid: &'a str) -> SessionSecurityFuture<'a, bool>;

    fn revoke_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> SessionSecurityFuture<'a, bool>;

    fn revoke_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        sid: &'a str,
    ) -> SessionSecurityFuture<'a, SessionRevocation>;

    fn session_sids_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> SessionSecurityFuture<'a, Vec<String>>;

    fn revoke_other_sessions_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        current_sid: &'a str,
        candidate_sids: &'a [String],
    ) -> SessionSecurityFuture<'a, u64>;
}
