use std::sync::Arc;

use ryframe_adapters::{
    RefreshFamily, RefreshRotation, RefreshSessionIdentity as AdapterIdentity,
    RefreshSessionRevocation as AdapterRevocation, RefreshSessionStore,
};
use ryframe_application::ports::auth::{
    RefreshSessionFamily, RefreshSessionIdentity, RefreshSessionPort, RefreshSessionRevocation,
    RefreshSessionRotation,
};

struct RefreshSessionBridge {
    store: RefreshSessionStore,
}

#[async_trait::async_trait]
impl RefreshSessionPort for RefreshSessionBridge {
    async fn register(&self, family: RefreshSessionFamily) -> ryframe_kernel::AppResult<()> {
        self.store
            .register(RefreshFamily {
                sid: family.sid,
                tenant_id: family.tenant_id,
                user_id: family.user_id,
                current_jti: family.current_jti,
                previous_jti: family.previous_jti,
                last_attempt_id: family.last_attempt_id,
                rotated_at: family.rotated_at,
                absolute_exp: family.absolute_exp,
                revoked: family.revoked,
            })
            .await
    }

    async fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> ryframe_kernel::AppResult<RefreshSessionRotation> {
        self.store
            .rotate(sid, presented_jti, new_jti, now, attempt_id)
            .await
            .map(map_rotation)
    }

    async fn identity(
        &self,
        sid: &str,
    ) -> ryframe_kernel::AppResult<Option<RefreshSessionIdentity>> {
        self.store
            .identity(sid)
            .await
            .map(|identity| identity.map(map_identity))
    }

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
    ) -> ryframe_kernel::AppResult<RefreshSessionRevocation> {
        self.store
            .revoke_for_user(tenant_id, user_id, sid)
            .await
            .map(map_revocation)
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

pub fn store(redis: Option<ryframe_adapters::RedisClient>) -> Arc<dyn RefreshSessionPort> {
    let store = RefreshSessionStore::new(redis);
    Arc::new(RefreshSessionBridge { store })
}

fn map_identity(identity: AdapterIdentity) -> RefreshSessionIdentity {
    RefreshSessionIdentity {
        tenant_id: identity.tenant_id,
        user_id: identity.user_id,
        absolute_exp: identity.absolute_exp,
    }
}

fn map_revocation(revocation: AdapterRevocation) -> RefreshSessionRevocation {
    match revocation {
        AdapterRevocation::Revoked => RefreshSessionRevocation::Revoked,
        AdapterRevocation::AlreadyRevoked => RefreshSessionRevocation::AlreadyRevoked,
        AdapterRevocation::NotFoundOrForeign => RefreshSessionRevocation::NotFoundOrForeign,
    }
}

pub fn map_rotation(rotation: RefreshRotation) -> RefreshSessionRotation {
    match rotation {
        RefreshRotation::Rotated {
            current_jti,
            issued_at,
        } => RefreshSessionRotation::Rotated {
            current_jti,
            issued_at,
        },
        RefreshRotation::Recovered {
            current_jti,
            issued_at,
        } => RefreshSessionRotation::Recovered {
            current_jti,
            issued_at,
        },
        RefreshRotation::Concurrent => RefreshSessionRotation::Concurrent,
        RefreshRotation::Replayed => RefreshSessionRotation::Replayed,
        RefreshRotation::MissingOrRevoked => RefreshSessionRotation::MissingOrRevoked,
    }
}
