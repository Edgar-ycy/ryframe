use std::sync::Arc;

use ryframe_adapters::TokenBlacklist;
use ryframe_api::session_security::{
    AccessRevocationStore, RefreshSessionControl, SessionRevocation, SessionSecurityFuture,
};
use ryframe_application::{
    RefreshSessionPort, RefreshSessionRevocation as ApplicationSessionRevocation,
};

struct AccessRevocationStoreBridge {
    store: TokenBlacklist,
}

impl AccessRevocationStore for AccessRevocationStoreBridge {
    fn is_revoked<'a>(&'a self, jti: &'a str) -> SessionSecurityFuture<'a, bool> {
        Box::pin(async move { self.store.try_is_blacklisted(jti).await })
    }

    fn revoke<'a>(&'a self, jti: &'a str, ttl_seconds: u64) -> SessionSecurityFuture<'a, ()> {
        Box::pin(async move { self.store.try_blacklist(jti, ttl_seconds).await })
    }
}

struct RefreshSessionControlBridge {
    store: Arc<dyn RefreshSessionPort>,
}

impl RefreshSessionControl for RefreshSessionControlBridge {
    fn is_active_for_identity<'a>(
        &'a self,
        sid: &'a str,
        tenant_id: &'a str,
        user_id: i64,
    ) -> SessionSecurityFuture<'a, bool> {
        Box::pin(async move {
            self.store
                .is_active_for_identity(sid, tenant_id, user_id)
                .await
        })
    }

    fn revoke<'a>(&'a self, sid: &'a str) -> SessionSecurityFuture<'a, bool> {
        Box::pin(async move { self.store.revoke(sid).await })
    }

    fn revoke_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> SessionSecurityFuture<'a, bool> {
        Box::pin(async move { self.store.revoke_for_tenant(tenant_id, sid).await })
    }

    fn revoke_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        sid: &'a str,
    ) -> SessionSecurityFuture<'a, SessionRevocation> {
        Box::pin(async move {
            self.store
                .revoke_for_user(tenant_id, user_id, sid)
                .await
                .map(map_session_revocation)
        })
    }

    fn session_sids_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> SessionSecurityFuture<'a, Vec<String>> {
        Box::pin(async move { self.store.session_sids_for_user(tenant_id, user_id).await })
    }

    fn revoke_other_sessions_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        current_sid: &'a str,
        candidate_sids: &'a [String],
    ) -> SessionSecurityFuture<'a, u64> {
        Box::pin(async move {
            self.store
                .revoke_other_sessions_for_user(tenant_id, user_id, current_sid, candidate_sids)
                .await
        })
    }
}

pub fn access_revocations(store: TokenBlacklist) -> Arc<dyn AccessRevocationStore> {
    Arc::new(AccessRevocationStoreBridge { store })
}

pub fn refresh_sessions(store: Arc<dyn RefreshSessionPort>) -> Arc<dyn RefreshSessionControl> {
    Arc::new(RefreshSessionControlBridge { store })
}

const fn map_session_revocation(value: ApplicationSessionRevocation) -> SessionRevocation {
    match value {
        ApplicationSessionRevocation::Revoked => SessionRevocation::Revoked,
        ApplicationSessionRevocation::AlreadyRevoked => SessionRevocation::AlreadyRevoked,
        ApplicationSessionRevocation::NotFoundOrForeign => SessionRevocation::NotFoundOrForeign,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_revocation_mapping_is_complete() {
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::Revoked),
            SessionRevocation::Revoked
        );
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::AlreadyRevoked),
            SessionRevocation::AlreadyRevoked
        );
        assert_eq!(
            map_session_revocation(ApplicationSessionRevocation::NotFoundOrForeign),
            SessionRevocation::NotFoundOrForeign
        );
    }
}
