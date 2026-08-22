use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use tokio::sync::RwLock;

use super::{
    OnlineSessionMetadataFuture, OnlineSessionMetadataStore, UserSession, keyspace::session_key,
    remaining_ttl,
};

type Sessions = Arc<RwLock<HashMap<String, UserSession>>>;

async fn add(sessions: &Sessions, session: UserSession) {
    sessions
        .write()
        .await
        .insert(session_key(&session.tenant_id, &session.sid), session);
}

async fn remove(sessions: &Sessions, tenant_id: &str, sid: &str) {
    sessions.write().await.remove(&session_key(tenant_id, sid));
}

async fn list(sessions: &Sessions, tenant_id: &str) -> Vec<UserSession> {
    sessions
        .read()
        .await
        .values()
        .filter(|session| {
            session.tenant_id == tenant_id && remaining_ttl(session.absolute_exp).is_some()
        })
        .cloned()
        .collect()
}

async fn list_for_user(sessions: &Sessions, tenant_id: &str, user_id: i64) -> Vec<UserSession> {
    sessions
        .read()
        .await
        .values()
        .filter(|session| {
            session.tenant_id == tenant_id
                && session.user_id == user_id
                && remaining_ttl(session.absolute_exp).is_some()
        })
        .cloned()
        .collect()
}

async fn touch(sessions: &Sessions, tenant_id: &str, sid: &str) -> bool {
    let key = session_key(tenant_id, sid);
    let mut sessions = sessions.write().await;
    let expired = sessions
        .get(&key)
        .is_some_and(|session| remaining_ttl(session.absolute_exp).is_none());
    if expired {
        sessions.remove(&key);
        false
    } else if let Some(session) = sessions.get_mut(&key) {
        session.last_access_time = Utc::now();
        true
    } else {
        false
    }
}

async fn cleanup_expired(sessions: &Sessions) {
    sessions
        .write()
        .await
        .retain(|_, session| remaining_ttl(session.absolute_exp).is_some());
}

#[derive(Default)]
pub(super) struct InMemoryOnlineSessionMetadata {
    sessions: Sessions,
}

impl OnlineSessionMetadataStore for InMemoryOnlineSessionMetadata {
    fn add(&self, session: UserSession, _ttl_seconds: u64) -> OnlineSessionMetadataFuture<'_, ()> {
        Box::pin(async move {
            add(&self.sessions, session).await;
            Ok(())
        })
    }

    fn remove<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> OnlineSessionMetadataFuture<'a, ()> {
        Box::pin(async move {
            remove(&self.sessions, tenant_id, sid).await;
            Ok(())
        })
    }

    fn list<'a>(&'a self, tenant_id: &'a str) -> OnlineSessionMetadataFuture<'a, Vec<UserSession>> {
        Box::pin(async move { Ok(list(&self.sessions, tenant_id).await) })
    }

    fn list_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> OnlineSessionMetadataFuture<'a, Vec<UserSession>> {
        Box::pin(async move { Ok(list_for_user(&self.sessions, tenant_id, user_id).await) })
    }

    fn touch<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> OnlineSessionMetadataFuture<'a, bool> {
        Box::pin(async move { Ok(touch(&self.sessions, tenant_id, sid).await) })
    }

    fn cleanup_expired(&self) -> OnlineSessionMetadataFuture<'_, ()> {
        Box::pin(async move {
            cleanup_expired(&self.sessions).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(sid: &str, absolute_exp: i64) -> UserSession {
        let now = Utc::now();
        UserSession {
            sid: sid.into(),
            tenant_id: "tenant-a".into(),
            user_id: 42,
            username: "alice".into(),
            dept_name: None,
            ipaddr: "192.0.2.1".into(),
            login_location: None,
            browser: None,
            os: None,
            login_time: now,
            last_access_time: now,
            absolute_exp,
        }
    }

    #[tokio::test]
    async fn metadata_is_isolated_and_removable() {
        let store = InMemoryOnlineSessionMetadata::default();
        store
            .add(session("sid-a", Utc::now().timestamp() + 60), 60)
            .await
            .expect("应写入设备元数据");

        assert_eq!(
            store
                .list_for_user("tenant-a", 42)
                .await
                .expect("应读取用户设备")
                .len(),
            1
        );
        assert!(
            store
                .list("tenant-b")
                .await
                .expect("应隔离其他租户")
                .is_empty()
        );
        assert!(
            store
                .touch("tenant-a", "sid-a")
                .await
                .expect("应更新设备活动时间")
        );

        store
            .remove("tenant-a", "sid-a")
            .await
            .expect("应删除设备元数据");
        assert!(
            store
                .list("tenant-a")
                .await
                .expect("应读取租户设备")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn expired_metadata_is_not_returned() {
        let store = InMemoryOnlineSessionMetadata::default();
        store
            .add(session("sid-expired", Utc::now().timestamp() - 1), 1)
            .await
            .expect("应允许写入待清理元数据");
        store.cleanup_expired().await.expect("应清理过期元数据");
        assert!(
            store
                .list("tenant-a")
                .await
                .expect("应读取租户设备")
                .is_empty()
        );
    }
}
