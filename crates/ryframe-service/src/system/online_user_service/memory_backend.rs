use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use tokio::sync::RwLock;

use super::{UserSession, keyspace::session_key, session_codec::remaining_ttl};

pub(super) type Sessions = Arc<RwLock<HashMap<String, UserSession>>>;

pub(super) async fn add(sessions: &Sessions, session: UserSession) {
    sessions
        .write()
        .await
        .insert(session_key(&session.tenant_id, &session.sid), session);
}

pub(super) async fn remove(sessions: &Sessions, tenant_id: &str, sid: &str) {
    sessions.write().await.remove(&session_key(tenant_id, sid));
}

pub(super) async fn list(sessions: &Sessions, tenant_id: &str) -> Vec<UserSession> {
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

pub(super) async fn list_for_user(
    sessions: &Sessions,
    tenant_id: &str,
    user_id: i64,
) -> Vec<UserSession> {
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

pub(super) async fn touch(sessions: &Sessions, tenant_id: &str, sid: &str) -> bool {
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

pub(super) async fn cleanup_expired(sessions: &Sessions) {
    sessions
        .write()
        .await
        .retain(|_, session| remaining_ttl(session.absolute_exp).is_some());
}
