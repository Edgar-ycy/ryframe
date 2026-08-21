use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type RefreshSessionFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

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
pub trait RefreshSessionPort: Send + Sync {
    fn register(&self, family: RefreshSessionFamily) -> RefreshSessionFuture<'_, ()>;

    fn rotate<'a>(
        &'a self,
        sid: &'a str,
        presented_jti: &'a str,
        new_jti: &'a str,
        now: i64,
        attempt_id: &'a str,
    ) -> RefreshSessionFuture<'a, RefreshSessionRotation>;

    fn identity<'a>(
        &'a self,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, Option<RefreshSessionIdentity>>;

    fn is_active_for_identity<'a>(
        &'a self,
        sid: &'a str,
        tenant_id: &'a str,
        user_id: i64,
    ) -> RefreshSessionFuture<'a, bool>;

    fn revoke<'a>(&'a self, sid: &'a str) -> RefreshSessionFuture<'a, bool>;

    fn revoke_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, bool>;

    fn revoke_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, RefreshSessionRevocation>;

    fn session_sids_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> RefreshSessionFuture<'a, Vec<String>>;

    fn revoke_other_sessions_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        current_sid: &'a str,
        candidate_sids: &'a [String],
    ) -> RefreshSessionFuture<'a, u64>;
}
