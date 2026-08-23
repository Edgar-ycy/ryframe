use std::sync::Arc;

use ryframe_adapters::TokenBlacklist;
use ryframe_api::session_security::{
    AccessRevocationStore, RefreshSessionControl, SessionRevocation,
};
use ryframe_application::ports::auth::{
    RefreshSessionPort, RefreshSessionRevocation as ApplicationSessionRevocation,
};

struct AccessRevocationStoreBridge {
    store: TokenBlacklist,
}

#[async_trait::async_trait]
impl AccessRevocationStore for AccessRevocationStoreBridge {
    async fn is_revoked(&self, jti: &str) -> ryframe_kernel::AppResult<bool> {
        self.store.try_is_blacklisted(jti).await
    }

    async fn revoke(&self, jti: &str, ttl_seconds: u64) -> ryframe_kernel::AppResult<()> {
        self.store.try_blacklist(jti, ttl_seconds).await
    }
}

struct RefreshSessionControlBridge {
    store: Arc<dyn RefreshSessionPort>,
}

#[async_trait::async_trait]
impl RefreshSessionControl for RefreshSessionControlBridge {
    async fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<bool> {
        self.store
            .is_active_for_identity(sid, tenant_id, user_id)
            .await
    }

    async fn revoke(&self, sid: &str) -> ryframe_kernel::AppResult<bool> {
        self.store.revoke(sid).await
    }

    async fn revoke_for_tenant(
        &self,
        tenant_id: &str,
        sid: &str,
    ) -> ryframe_kernel::AppResult<bool> {
        self.store.revoke_for_tenant(tenant_id, sid).await
    }

    async fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
    ) -> ryframe_kernel::AppResult<SessionRevocation> {
        self.store
            .revoke_for_user(tenant_id, user_id, sid)
            .await
            .map(map_session_revocation)
    }

    async fn session_sids_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<String>> {
        self.store.session_sids_for_user(tenant_id, user_id).await
    }

    async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidate_sids: &[String],
    ) -> ryframe_kernel::AppResult<u64> {
        self.store
            .revoke_other_sessions_for_user(tenant_id, user_id, current_sid, candidate_sids)
            .await
    }
}

pub fn access_revocations(store: TokenBlacklist) -> Arc<dyn AccessRevocationStore> {
    Arc::new(AccessRevocationStoreBridge { store })
}

pub fn refresh_sessions(store: Arc<dyn RefreshSessionPort>) -> Arc<dyn RefreshSessionControl> {
    Arc::new(RefreshSessionControlBridge { store })
}

pub const fn map_session_revocation(value: ApplicationSessionRevocation) -> SessionRevocation {
    match value {
        ApplicationSessionRevocation::Revoked => SessionRevocation::Revoked,
        ApplicationSessionRevocation::AlreadyRevoked => SessionRevocation::AlreadyRevoked,
        ApplicationSessionRevocation::NotFoundOrForeign => SessionRevocation::NotFoundOrForeign,
    }
}
