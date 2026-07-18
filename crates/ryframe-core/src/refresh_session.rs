//! Refresh-token family state.
//!
//! A family is keyed by a stable session id (`sid`). Redis mode uses one Lua
//! script for compare-and-swap rotation so two application instances cannot
//! both accept the same refresh token.

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use ryframe_common::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::RedisClient;

const KEY_PREFIX: &str = "ryframe:v0.5:refresh-family:";
const CONCURRENT_GRACE_SECONDS: i64 = 5;
static LOCAL_FAMILIES: OnceLock<Arc<DashMap<String, RefreshFamily>>> = OnceLock::new();

const ROTATE_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {0, '', 0} end
if redis.call('HGET', KEYS[1], 'revoked') == '1' then return {0, '', 0} end
local current = redis.call('HGET', KEYS[1], 'current_jti')
local previous = redis.call('HGET', KEYS[1], 'previous_jti')
local last_attempt = redis.call('HGET', KEYS[1], 'last_attempt_id')
local rotated_at = tonumber(redis.call('HGET', KEYS[1], 'rotated_at') or '0')
local absolute_exp = tonumber(redis.call('HGET', KEYS[1], 'absolute_exp') or '0')
local now = tonumber(ARGV[3])
if absolute_exp <= now then
  redis.call('DEL', KEYS[1])
  return {0, '', 0}
end
if current == ARGV[1] then
  redis.call('HSET', KEYS[1],
    'previous_jti', current,
    'current_jti', ARGV[2],
    'rotated_at', ARGV[3],
    'last_attempt_id', ARGV[5])
  redis.call('EXPIREAT', KEYS[1], absolute_exp)
  return {1, ARGV[2], now}
end
if previous == ARGV[1] then
  if last_attempt == ARGV[5] then
    return {4, current, rotated_at}
  end
  if now - rotated_at <= tonumber(ARGV[4]) then
    return {2, '', 0}
  end
end
redis.call('HSET', KEYS[1], 'revoked', '1')
redis.call('EXPIREAT', KEYS[1], absolute_exp)
return {3, '', 0}
"#;

const REGISTER_SCRIPT: &str = r#"
redis.call('HSET', KEYS[1],
  'sid', ARGV[1],
  'tenant_id', ARGV[2],
  'user_id', ARGV[3],
  'current_jti', ARGV[4],
  'previous_jti', ARGV[5],
  'rotated_at', ARGV[6],
  'absolute_exp', ARGV[7],
  'revoked', ARGV[8],
  'last_attempt_id', ARGV[9])
redis.call('EXPIREAT', KEYS[1], tonumber(ARGV[7]))
return 1
"#;

// The refresh family is the authority for force logout.  Keep the tenant
// check and revocation in one Redis operation so an online-user index entry
// can never authorize revoking another tenant's session.
const REVOKE_FOR_TENANT_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end
if redis.call('HGET', KEYS[1], 'tenant_id') ~= ARGV[1] then return 0 end
redis.call('HSET', KEYS[1], 'revoked', '1')
return 1
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshFamily {
    pub sid: String,
    pub tenant_id: String,
    pub user_id: i64,
    pub current_jti: String,
    pub previous_jti: Option<String>,
    #[serde(default)]
    pub last_attempt_id: Option<String>,
    pub rotated_at: i64,
    pub absolute_exp: i64,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRotation {
    Rotated { current_jti: String, issued_at: i64 },
    Recovered { current_jti: String, issued_at: i64 },
    Concurrent,
    Replayed,
    MissingOrRevoked,
}

#[derive(Clone)]
pub struct RefreshSessionStore {
    redis: Option<RedisClient>,
    local: Arc<DashMap<String, RefreshFamily>>,
}

impl RefreshSessionStore {
    pub fn new(redis: Option<RedisClient>) -> Self {
        Self {
            redis,
            local: LOCAL_FAMILIES
                .get_or_init(|| Arc::new(DashMap::new()))
                .clone(),
        }
    }

    pub fn is_distributed(&self) -> bool {
        self.redis.is_some()
    }

    pub async fn register(&self, family: RefreshFamily) -> AppResult<()> {
        remaining_ttl(family.absolute_exp)?;
        if let Some(redis) = &self.redis {
            let key = family_key(&family.sid);
            let previous = family.previous_jti.as_deref().unwrap_or("");
            let user_id = family.user_id.to_string();
            let rotated_at = family.rotated_at.to_string();
            let absolute_exp = family.absolute_exp.to_string();
            redis
                .eval_script(
                    REGISTER_SCRIPT,
                    &[key.as_str()],
                    &[
                        family.sid.as_str(),
                        family.tenant_id.as_str(),
                        user_id.as_str(),
                        family.current_jti.as_str(),
                        previous,
                        rotated_at.as_str(),
                        absolute_exp.as_str(),
                        if family.revoked { "1" } else { "0" },
                        family.last_attempt_id.as_deref().unwrap_or(""),
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
        } else {
            self.local.insert(family.sid.clone(), family);
        }
        Ok(())
    }

    pub async fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> AppResult<RefreshRotation> {
        if attempt_id.is_empty() {
            return Err(AppError::Authorization(
                "missing refresh rotation attempt id".into(),
            ));
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let now = now.to_string();
            let grace = CONCURRENT_GRACE_SECONDS.to_string();
            let result = redis
                .eval_script(
                    ROTATE_SCRIPT,
                    &[key.as_str()],
                    &[
                        presented_jti,
                        new_jti,
                        now.as_str(),
                        grace.as_str(),
                        attempt_id,
                    ],
                )
                .await
                .map_err(redis_unavailable)?;
            let (code, current_jti, issued_at): (i64, String, i64) =
                redis::from_redis_value(&result).map_err(redis_unavailable)?;
            return Ok(match code {
                1 => RefreshRotation::Rotated {
                    current_jti,
                    issued_at,
                },
                2 => RefreshRotation::Concurrent,
                3 => RefreshRotation::Replayed,
                4 => RefreshRotation::Recovered {
                    current_jti,
                    issued_at,
                },
                _ => RefreshRotation::MissingOrRevoked,
            });
        }

        let Some(mut family) = self.local.get_mut(sid) else {
            return Ok(RefreshRotation::MissingOrRevoked);
        };
        if family.revoked || family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(RefreshRotation::MissingOrRevoked);
        }
        if family.current_jti == presented_jti {
            family.previous_jti = Some(family.current_jti.clone());
            family.current_jti = new_jti.to_owned();
            family.rotated_at = now;
            family.last_attempt_id = Some(attempt_id.to_owned());
            return Ok(RefreshRotation::Rotated {
                current_jti: new_jti.to_owned(),
                issued_at: now,
            });
        }
        if family.previous_jti.as_deref() == Some(presented_jti) {
            if family.last_attempt_id.as_deref() == Some(attempt_id) {
                return Ok(RefreshRotation::Recovered {
                    current_jti: family.current_jti.clone(),
                    issued_at: family.rotated_at,
                });
            }
            if now - family.rotated_at <= CONCURRENT_GRACE_SECONDS {
                return Ok(RefreshRotation::Concurrent);
            }
        }
        family.revoked = true;
        Ok(RefreshRotation::Replayed)
    }

    pub async fn revoke(&self, sid: &str) -> AppResult<bool> {
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let script = "if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end; redis.call('HSET', KEYS[1], 'revoked', '1'); return 1";
            let result = redis
                .eval_script(script, &[key.as_str()], &[] as &[&str])
                .await
                .map_err(redis_unavailable)?;
            return redis::from_redis_value(&result).map_err(redis_unavailable);
        }
        if let Some(mut family) = self.local.get_mut(sid) {
            family.revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Idempotently revoke a refresh family after atomically verifying that
    /// it belongs to `tenant_id`.
    ///
    /// `false` intentionally covers both a missing family and a tenant
    /// mismatch, so callers cannot use this operation to enumerate another
    /// tenant's sessions. Redis failures remain distinguishable as 503s.
    pub async fn revoke_for_tenant(&self, tenant_id: &str, sid: &str) -> AppResult<bool> {
        if tenant_id.is_empty() || sid.is_empty() {
            return Ok(false);
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let result = redis
                .eval_script(REVOKE_FOR_TENANT_SCRIPT, &[key.as_str()], &[tenant_id])
                .await
                .map_err(redis_unavailable)?;
            let code: i64 = redis::from_redis_value(&result).map_err(redis_unavailable)?;
            return Ok(code == 1);
        }

        let now = chrono::Utc::now().timestamp();
        let Some(mut family) = self.local.get_mut(sid) else {
            return Ok(false);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.local.remove(sid);
            return Ok(false);
        }
        if family.tenant_id != tenant_id {
            return Ok(false);
        }
        family.revoked = true;
        Ok(true)
    }

    pub async fn is_active(&self, sid: &str) -> AppResult<bool> {
        if sid.is_empty() {
            return Ok(false);
        }
        if let Some(redis) = &self.redis {
            let key = family_key(sid);
            let script = "if redis.call('EXISTS', KEYS[1]) == 0 then return 0 end; if redis.call('HGET', KEYS[1], 'revoked') == '1' then return 0 end; return 1";
            let result = redis
                .eval_script(script, &[key.as_str()], &[] as &[&str])
                .await
                .map_err(redis_unavailable)?;
            return redis::from_redis_value(&result).map_err(redis_unavailable);
        }
        let now = chrono::Utc::now().timestamp();
        Ok(self
            .local
            .get(sid)
            .is_some_and(|family| !family.revoked && family.absolute_exp > now))
    }
}

fn family_key(sid: &str) -> String {
    format!("{KEY_PREFIX}{sid}")
}

fn remaining_ttl(absolute_exp: i64) -> AppResult<u64> {
    let remaining = absolute_exp - chrono::Utc::now().timestamp();
    if remaining <= 0 {
        return Err(AppError::Authentication("refresh session expired".into()));
    }
    Ok(remaining as u64)
}

fn redis_unavailable(error: redis::RedisError) -> AppError {
    tracing::error!(%error, "refresh session Redis operation failed");
    AppError::ServiceUnavailable("session service unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(sid: &str) -> RefreshFamily {
        RefreshFamily {
            sid: sid.into(),
            tenant_id: "system".into(),
            user_id: 1,
            current_jti: "jti-1".into(),
            previous_jti: None,
            last_attempt_id: None,
            rotated_at: 0,
            absolute_exp: chrono::Utc::now().timestamp() + 600,
            revoked: false,
        }
    }

    #[tokio::test]
    async fn local_rotation_detects_concurrency_and_replay() {
        let store = RefreshSessionStore::new(None);
        store.register(family("sid-1")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        assert_eq!(
            store
                .rotate("sid-1", "jti-1", "jti-2", now, "attempt-1")
                .await
                .unwrap(),
            RefreshRotation::Rotated {
                current_jti: "jti-2".into(),
                issued_at: now,
            }
        );
        assert_eq!(
            store
                .rotate("sid-1", "jti-1", "jti-3", now + 1, "attempt-2")
                .await
                .unwrap(),
            RefreshRotation::Concurrent
        );
        assert_eq!(
            store
                .rotate("sid-1", "jti-1", "jti-3", now + 6, "attempt-3")
                .await
                .unwrap(),
            RefreshRotation::Replayed
        );
        assert!(!store.is_active("sid-1").await.unwrap());
    }

    #[tokio::test]
    async fn local_rotation_allows_only_one_concurrent_winner() {
        let store = Arc::new(RefreshSessionStore::new(None));
        store.register(family("sid-concurrent")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut tasks = Vec::new();
        for index in 0..50 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .rotate(
                        "sid-concurrent",
                        "jti-1",
                        &format!("jti-new-{index}"),
                        now,
                        &format!("attempt-{index}"),
                    )
                    .await
                    .unwrap()
            }));
        }

        let mut rotated = 0;
        let mut concurrent = 0;
        for task in tasks {
            match task.await.unwrap() {
                RefreshRotation::Rotated { .. } => rotated += 1,
                RefreshRotation::Concurrent => concurrent += 1,
                unexpected => panic!("unexpected rotation result: {unexpected:?}"),
            }
        }
        assert_eq!(rotated, 1);
        assert_eq!(concurrent, 49);
    }

    #[tokio::test]
    async fn same_rotation_attempt_recovers_the_committed_result() {
        let store = RefreshSessionStore::new(None);
        store.register(family("sid-recover")).await.unwrap();
        let now = chrono::Utc::now().timestamp();
        assert!(matches!(
            store
                .rotate("sid-recover", "jti-1", "jti-committed", now, "csrf-attempt")
                .await
                .unwrap(),
            RefreshRotation::Rotated { .. }
        ));

        assert_eq!(
            store
                .rotate(
                    "sid-recover",
                    "jti-1",
                    "jti-must-not-replace-committed",
                    now + 30,
                    "csrf-attempt",
                )
                .await
                .unwrap(),
            RefreshRotation::Recovered {
                current_jti: "jti-committed".into(),
                issued_at: now,
            }
        );
        assert!(store.is_active("sid-recover").await.unwrap());
    }

    #[tokio::test]
    async fn rotation_rejects_an_absolute_expiry_even_if_redis_ttl_lingers() {
        let store = RefreshSessionStore::new(None);
        let mut expiring = family("sid-absolute-expiry");
        let now = chrono::Utc::now().timestamp();
        expiring.absolute_exp = now + 1;
        store.register(expiring).await.unwrap();

        assert_eq!(
            store
                .rotate(
                    "sid-absolute-expiry",
                    "jti-1",
                    "jti-2",
                    now + 2,
                    "attempt-expired",
                )
                .await
                .unwrap(),
            RefreshRotation::MissingOrRevoked
        );
        assert!(!store.is_active("sid-absolute-expiry").await.unwrap());
    }

    #[tokio::test]
    async fn expired_family_is_rejected_at_registration() {
        let store = RefreshSessionStore::new(None);
        let mut expired = family("sid-expired");
        expired.absolute_exp = chrono::Utc::now().timestamp() - 1;
        assert!(matches!(
            store.register(expired).await,
            Err(AppError::Authentication(_))
        ));
        assert!(!store.is_active("sid-expired").await.unwrap());
    }

    #[tokio::test]
    async fn tenant_scoped_revocation_is_safe_and_idempotent() {
        let store = RefreshSessionStore::new(None);
        store.register(family("sid-force-logout")).await.unwrap();

        assert!(
            !store
                .revoke_for_tenant("another-tenant", "sid-force-logout")
                .await
                .unwrap()
        );
        assert!(store.is_active("sid-force-logout").await.unwrap());

        assert!(
            store
                .revoke_for_tenant("system", "sid-force-logout")
                .await
                .unwrap()
        );
        assert!(
            store
                .revoke_for_tenant("system", "sid-force-logout")
                .await
                .unwrap()
        );
        assert!(!store.is_active("sid-force-logout").await.unwrap());
    }

    #[tokio::test]
    async fn revoking_one_device_does_not_revoke_a_sibling_session() {
        let store = RefreshSessionStore::new(None);
        let mut first_device = family("sid-device-a");
        first_device.current_jti = "jti-device-a".into();
        let mut second_device = family("sid-device-b");
        second_device.current_jti = "jti-device-b".into();
        store.register(first_device).await.unwrap();
        store.register(second_device).await.unwrap();

        assert!(
            store
                .revoke_for_tenant("system", "sid-device-a")
                .await
                .unwrap()
        );
        assert!(!store.is_active("sid-device-a").await.unwrap());
        assert!(store.is_active("sid-device-b").await.unwrap());
    }
}
