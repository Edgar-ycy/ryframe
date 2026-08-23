use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_kernel::AppResult;
use tokio::sync::RwLock;

use super::{OnlineSessionMetadataStore, UserSession, keyspace::session_key, remaining_ttl};

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

#[async_trait::async_trait]
impl OnlineSessionMetadataStore for InMemoryOnlineSessionMetadata {
    async fn add(&self, session: UserSession, _ttl_seconds: u64) -> AppResult<()> {
        add(&self.sessions, session).await;
        Ok(())
    }

    async fn remove(&self, tenant_id: &str, sid: &str) -> AppResult<()> {
        remove(&self.sessions, tenant_id, sid).await;
        Ok(())
    }

    async fn list(&self, tenant_id: &str) -> AppResult<Vec<UserSession>> {
        Ok(list(&self.sessions, tenant_id).await)
    }

    async fn list_for_user(&self, tenant_id: &str, user_id: i64) -> AppResult<Vec<UserSession>> {
        Ok(list_for_user(&self.sessions, tenant_id, user_id).await)
    }

    async fn touch(&self, tenant_id: &str, sid: &str) -> AppResult<bool> {
        Ok(touch(&self.sessions, tenant_id, sid).await)
    }

    async fn cleanup_expired(&self) -> AppResult<()> {
        cleanup_expired(&self.sessions).await;
        Ok(())
    }
}
