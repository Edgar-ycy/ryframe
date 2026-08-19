use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use dashmap::DashMap;
use ryframe_kernel::{AppError, AppResult};

use super::{
    CONCURRENT_GRACE_SECONDS, MAX_BULK_SESSION_CANDIDATES, RefreshFamily, RefreshRotation,
    RefreshSessionIdentity, RefreshSessionRevocation,
};

static FAMILIES: OnceLock<Arc<DashMap<String, RefreshFamily>>> = OnceLock::new();
static MUTATION_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

#[derive(Clone)]
pub(super) struct MemoryRefreshSessionStore {
    families: Arc<DashMap<String, RefreshFamily>>,
    mutation_lock: Arc<Mutex<()>>,
}

impl MemoryRefreshSessionStore {
    pub(super) fn shared() -> Self {
        Self {
            families: FAMILIES.get_or_init(|| Arc::new(DashMap::new())).clone(),
            mutation_lock: MUTATION_LOCK
                .get_or_init(|| Arc::new(Mutex::new(())))
                .clone(),
        }
    }

    fn lock_mutations(&self) -> AppResult<MutexGuard<'_, ()>> {
        self.mutation_lock.lock().map_err(|error| {
            tracing::error!(%error, "本地会话变更锁已损坏");
            AppError::ServiceUnavailable("session service unavailable".into())
        })
    }

    pub(super) fn register(&self, family: RefreshFamily) -> AppResult<()> {
        let _mutation_guard = self.lock_mutations()?;
        let now = chrono::Utc::now().timestamp();
        let already_active = self.families.get(&family.sid).is_some_and(|existing| {
            !existing.revoked
                && existing.absolute_exp > now
                && existing.tenant_id == family.tenant_id
                && existing.user_id == family.user_id
        });
        if !family.revoked && !already_active {
            let active_count = self
                .families
                .iter()
                .filter(|entry| {
                    let existing = entry.value();
                    !existing.revoked
                        && existing.absolute_exp > now
                        && existing.tenant_id == family.tenant_id
                        && existing.user_id == family.user_id
                })
                .take(MAX_BULK_SESSION_CANDIDATES)
                .count();
            if active_count >= MAX_BULK_SESSION_CANDIDATES {
                return Err(AppError::Conflict("登录设备数量已达到安全上限".into()));
            }
        }
        self.families.insert(family.sid.clone(), family);
        Ok(())
    }

    pub(super) fn rotate(
        &self,
        sid: &str,
        presented_jti: &str,
        new_jti: &str,
        now: i64,
        attempt_id: &str,
    ) -> AppResult<RefreshRotation> {
        let _mutation_guard = self.lock_mutations()?;
        let Some(mut family) = self.families.get_mut(sid) else {
            return Ok(RefreshRotation::MissingOrRevoked);
        };
        if family.revoked || family.absolute_exp <= now {
            drop(family);
            self.families.remove(sid);
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

    pub(super) fn revoke(&self, sid: &str, now: i64) -> AppResult<bool> {
        let _mutation_guard = self.lock_mutations()?;
        if let Some(mut family) = self.families.get_mut(sid) {
            if family.absolute_exp <= now {
                drop(family);
                self.families.remove(sid);
                return Ok(false);
            }
            family.revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn revoke_for_tenant(
        &self,
        tenant_id: &str,
        sid: &str,
        now: i64,
    ) -> AppResult<bool> {
        let _mutation_guard = self.lock_mutations()?;
        let Some(mut family) = self.families.get_mut(sid) else {
            return Ok(false);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.families.remove(sid);
            return Ok(false);
        }
        if family.tenant_id != tenant_id {
            return Ok(false);
        }
        family.revoked = true;
        Ok(true)
    }

    pub(super) fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
        now: i64,
    ) -> AppResult<RefreshSessionRevocation> {
        let _mutation_guard = self.lock_mutations()?;
        let Some(mut family) = self.families.get_mut(sid) else {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.families.remove(sid);
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        if family.tenant_id != tenant_id || family.user_id != user_id {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        if family.revoked {
            return Ok(RefreshSessionRevocation::AlreadyRevoked);
        }
        family.revoked = true;
        Ok(RefreshSessionRevocation::Revoked)
    }

    pub(super) fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        now: i64,
    ) -> AppResult<u64> {
        let _mutation_guard = self.lock_mutations()?;
        let mut authoritative_sids: Vec<String> = self
            .families
            .iter()
            .filter_map(|entry| {
                let family = entry.value();
                (!family.revoked
                    && family.absolute_exp > now
                    && family.tenant_id == tenant_id
                    && family.user_id == user_id
                    && family.sid != current_sid)
                    .then(|| family.sid.clone())
            })
            .collect();
        authoritative_sids.sort_unstable();
        authoritative_sids.dedup();
        if authoritative_sids.len() > MAX_BULK_SESSION_CANDIDATES {
            return Err(AppError::Validation(format!(
                "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
            )));
        }
        let mut revoked_count = 0_u64;
        let mut expired = Vec::new();
        for sid in authoritative_sids {
            let Some(mut family) = self.families.get_mut(&sid) else {
                continue;
            };
            if family.tenant_id != tenant_id || family.user_id != user_id {
                continue;
            }
            if family.absolute_exp <= now {
                expired.push(sid);
            } else if !family.revoked {
                family.revoked = true;
                revoked_count += 1;
            }
        }
        for sid in expired {
            self.families.remove(&sid);
        }
        Ok(revoked_count)
    }

    pub(super) fn identity(
        &self,
        sid: &str,
        now: i64,
    ) -> AppResult<Option<RefreshSessionIdentity>> {
        let _mutation_guard = self.lock_mutations()?;
        let Some(family) = self.families.get(sid) else {
            return Ok(None);
        };
        if family.absolute_exp <= now {
            drop(family);
            self.families.remove(sid);
            return Ok(None);
        }
        if family.revoked {
            return Ok(None);
        }
        Ok(Some(RefreshSessionIdentity {
            tenant_id: family.tenant_id.clone(),
            user_id: family.user_id,
            absolute_exp: family.absolute_exp,
        }))
    }

    pub(super) fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
        now: i64,
    ) -> AppResult<bool> {
        let _mutation_guard = self.lock_mutations()?;
        let Some(family) = self.families.get(sid) else {
            return Ok(false);
        };
        let active = !family.revoked
            && family.absolute_exp > now
            && family.tenant_id == tenant_id
            && family.user_id == user_id;
        let expired = family.absolute_exp <= now;
        drop(family);
        if expired {
            self.families.remove(sid);
        }
        Ok(active)
    }

    pub(super) fn session_sids_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        now: i64,
    ) -> AppResult<Vec<String>> {
        let _mutation_guard = self.lock_mutations()?;
        Ok(self.session_sids(now, |family| {
            family.tenant_id == tenant_id && family.user_id == user_id
        }))
    }

    pub(super) fn session_sids_for_tenant(
        &self,
        tenant_id: &str,
        now: i64,
    ) -> AppResult<Vec<String>> {
        let _mutation_guard = self.lock_mutations()?;
        Ok(self.session_sids(now, |family| family.tenant_id == tenant_id))
    }

    fn session_sids(&self, now: i64, predicate: impl Fn(&RefreshFamily) -> bool) -> Vec<String> {
        let mut expired = Vec::new();
        let mut active = Vec::new();
        for entry in self.families.iter() {
            let family = entry.value();
            if family.absolute_exp <= now {
                expired.push(entry.key().clone());
            } else if !family.revoked && predicate(family) {
                active.push(entry.key().clone());
            }
        }
        for sid in expired {
            self.families.remove(&sid);
        }
        active.sort_unstable();
        active
    }
}
