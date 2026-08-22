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
pub struct InMemoryOnlineSessionMetadata {
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
