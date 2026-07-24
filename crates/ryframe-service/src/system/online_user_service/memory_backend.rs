use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use tokio::sync::RwLock;

use super::{
    OnlineUserVo, UserSession, keyspace::session_key, session_codec::remaining_ttl, session_to_vo,
};

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

pub(super) async fn list(sessions: &Sessions, tenant_id: &str) -> Vec<OnlineUserVo> {
    sessions
        .read()
        .await
        .values()
        .filter(|session| {
            session.tenant_id == tenant_id && remaining_ttl(session.absolute_exp).is_some()
        })
        .map(session_to_vo)
        .collect()
}

pub(super) async fn touch(sessions: &Sessions, tenant_id: &str, sid: &str) {
    let key = session_key(tenant_id, sid);
    let mut sessions = sessions.write().await;
    let expired = sessions
        .get(&key)
        .is_some_and(|session| remaining_ttl(session.absolute_exp).is_none());
    if expired {
        sessions.remove(&key);
    } else if let Some(session) = sessions.get_mut(&key) {
        session.last_access_time = Utc::now();
    }
}

pub(super) async fn cleanup_expired(sessions: &Sessions) {
    sessions
        .write()
        .await
        .retain(|_, session| remaining_ttl(session.absolute_exp).is_some());
}
