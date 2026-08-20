use std::sync::Arc;

use ryframe_adapters::{
    RefreshFamily, RefreshRotation, RefreshSessionIdentity as AdapterIdentity,
    RefreshSessionRevocation as AdapterRevocation, RefreshSessionStore,
};
use ryframe_application::{
    RefreshSessionFamily, RefreshSessionFuture, RefreshSessionIdentity, RefreshSessionPort,
    RefreshSessionRevocation, RefreshSessionRotation,
};

struct RefreshSessionBridge {
    store: RefreshSessionStore,
}

impl RefreshSessionPort for RefreshSessionBridge {
    fn register(&self, family: RefreshSessionFamily) -> RefreshSessionFuture<'_, ()> {
        Box::pin(async move {
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
        })
    }

    fn rotate<'a>(
        &'a self,
        sid: &'a str,
        presented_jti: &'a str,
        new_jti: &'a str,
        now: i64,
        attempt_id: &'a str,
    ) -> RefreshSessionFuture<'a, RefreshSessionRotation> {
        Box::pin(async move {
            self.store
                .rotate(sid, presented_jti, new_jti, now, attempt_id)
                .await
                .map(map_rotation)
        })
    }

    fn identity<'a>(
        &'a self,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, Option<RefreshSessionIdentity>> {
        Box::pin(async move {
            self.store
                .identity(sid)
                .await
                .map(|identity| identity.map(map_identity))
        })
    }

    fn is_active_for_identity<'a>(
        &'a self,
        sid: &'a str,
        tenant_id: &'a str,
        user_id: i64,
    ) -> RefreshSessionFuture<'a, bool> {
        Box::pin(async move {
            self.store
                .is_active_for_identity(sid, tenant_id, user_id)
                .await
        })
    }

    fn revoke<'a>(&'a self, sid: &'a str) -> RefreshSessionFuture<'a, bool> {
        Box::pin(async move { self.store.revoke(sid).await })
    }

    fn revoke_for_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, bool> {
        Box::pin(async move { self.store.revoke_for_tenant(tenant_id, sid).await })
    }

    fn revoke_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        sid: &'a str,
    ) -> RefreshSessionFuture<'a, RefreshSessionRevocation> {
        Box::pin(async move {
            self.store
                .revoke_for_user(tenant_id, user_id, sid)
                .await
                .map(map_revocation)
        })
    }

    fn session_sids_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> RefreshSessionFuture<'a, Vec<String>> {
        Box::pin(async move { self.store.session_sids_for_user(tenant_id, user_id).await })
    }

    fn revoke_other_sessions_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        current_sid: &'a str,
        candidate_sids: &'a [String],
    ) -> RefreshSessionFuture<'a, u64> {
        Box::pin(async move {
            self.store
                .revoke_other_sessions_for_user(tenant_id, user_id, current_sid, candidate_sids)
                .await
        })
    }
}

pub fn stores(
    redis: Option<ryframe_adapters::RedisClient>,
) -> (RefreshSessionStore, Arc<dyn RefreshSessionPort>) {
    let store = RefreshSessionStore::new(redis);
    let port = Arc::new(RefreshSessionBridge {
        store: store.clone(),
    });
    (store, port)
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

fn map_rotation(rotation: RefreshRotation) -> RefreshSessionRotation {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_refresh_rotation_outcomes() {
        assert_eq!(
            map_rotation(RefreshRotation::Concurrent),
            RefreshSessionRotation::Concurrent
        );
        assert_eq!(
            map_rotation(RefreshRotation::Replayed),
            RefreshSessionRotation::Replayed
        );
        assert_eq!(
            map_rotation(RefreshRotation::MissingOrRevoked),
            RefreshSessionRotation::MissingOrRevoked
        );
    }

    #[tokio::test]
    async fn memory_store_preserves_refresh_family_semantics() {
        let (_, sessions) = stores(None);
        let sid = ryframe_auth::jwt::new_sid();
        let now = chrono::Utc::now().timestamp();
        sessions
            .register(RefreshSessionFamily {
                sid: sid.clone(),
                tenant_id: "tenant-a".into(),
                user_id: 42,
                current_jti: "jti-1".into(),
                previous_jti: None,
                last_attempt_id: None,
                rotated_at: now,
                absolute_exp: now + 60,
                revoked: false,
            })
            .await
            .expect("应登记刷新会话族");

        let identity = sessions
            .identity(&sid)
            .await
            .expect("应读取刷新会话")
            .expect("刷新会话应存在");
        assert_eq!(identity.tenant_id, "tenant-a");
        assert_eq!(identity.user_id, 42);
        assert!(matches!(
            sessions
                .rotate(&sid, "jti-1", "jti-2", now + 1, "attempt-1")
                .await
                .expect("应轮换刷新令牌"),
            RefreshSessionRotation::Rotated { .. }
        ));
        assert!(sessions.revoke(&sid).await.expect("应撤销刷新会话"));
        assert!(
            sessions
                .identity(&sid)
                .await
                .expect("应读取撤销状态")
                .is_none()
        );
    }
}
