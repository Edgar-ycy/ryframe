//! 刷新令牌族状态。
//!
//! 一个令牌族由稳定会话标识（`sid`）定位。Redis 模式使用乐观事务进行原子比较与交换，
//! 进程内模式使用所有 Store 实例共享的状态和变更锁保持同等语义。

mod codec;
mod keyspace;
mod memory;
mod model;
mod redis_store;

use ryframe_kernel::{AppError, AppResult};

use crate::RedisClient;

use memory::MemoryRefreshSessionStore;
use redis_store::RedisRefreshSessionStore;

pub use model::{RefreshFamily, RefreshRotation, RefreshSessionIdentity, RefreshSessionRevocation};

const CONCURRENT_GRACE_SECONDS: i64 = 5;
const MAX_BULK_SESSION_CANDIDATES: usize = 256;

#[derive(Clone)]
pub struct RefreshSessionStore {
    redis: Option<RedisRefreshSessionStore>,
    memory: MemoryRefreshSessionStore,
}

impl RefreshSessionStore {
    pub fn new(redis: Option<RedisClient>) -> Self {
        Self {
            redis: redis.map(RedisRefreshSessionStore::new),
            memory: MemoryRefreshSessionStore::shared(),
        }
    }

    pub fn is_distributed(&self) -> bool {
        self.redis.is_some()
    }

    pub async fn register(&self, family: RefreshFamily) -> AppResult<()> {
        codec::ensure_not_expired(family.absolute_exp)?;
        match &self.redis {
            Some(redis) => redis.register(&family).await,
            None => self.memory.register(family),
        }
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
        match &self.redis {
            Some(redis) => {
                redis
                    .rotate(sid, presented_jti, new_jti, now, attempt_id)
                    .await
            }
            None => self
                .memory
                .rotate(sid, presented_jti, new_jti, now, attempt_id),
        }
    }

    pub async fn revoke(&self, sid: &str) -> AppResult<bool> {
        if sid.is_empty() {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.revoke(sid, now).await,
            None => self.memory.revoke(sid, now),
        }
    }

    /// 原子验证刷新令牌族属于 `tenant_id` 后，以幂等方式撤销该令牌族。
    ///
    /// `false` 有意同时涵盖令牌族不存在和租户不匹配，调用方不能借此枚举其他租户的会话。
    /// Redis 失败仍可区分为 503。
    pub async fn revoke_for_tenant(&self, tenant_id: &str, sid: &str) -> AppResult<bool> {
        if tenant_id.is_empty() || sid.is_empty() {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.revoke_for_tenant(tenant_id, sid, now).await,
            None => self.memory.revoke_for_tenant(tenant_id, sid, now),
        }
    }

    /// 按租户和用户校验会话归属并原子撤销。
    ///
    /// 不存在、已过期或属于其他身份的会话统一返回 `NotFoundOrForeign`，避免借此枚举会话。
    pub async fn revoke_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        sid: &str,
    ) -> AppResult<RefreshSessionRevocation> {
        if tenant_id.is_empty() || sid.is_empty() {
            return Ok(RefreshSessionRevocation::NotFoundOrForeign);
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.revoke_for_user(tenant_id, user_id, sid, now).await,
            None => self.memory.revoke_for_user(tenant_id, user_id, sid, now),
        }
    }

    /// 撤销指定用户除当前会话外的候选会话。
    ///
    /// Redis 模式在一个乐观事务内完成所有校验、撤销和核心索引清理，避免连接故障导致部分成功；
    /// 内存模式使用所有 Store 实例共享的变更锁，确保整批操作不与其他本地会话变更交错。
    pub async fn revoke_other_sessions_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
        current_sid: &str,
        candidate_sids: &[String],
    ) -> AppResult<u64> {
        if tenant_id.is_empty() || current_sid.is_empty() {
            return Err(AppError::Validation("会话身份不能为空".into()));
        }
        let mut candidates: Vec<&str> = candidate_sids
            .iter()
            .map(String::as_str)
            .filter(|sid| !sid.is_empty() && *sid != current_sid)
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.len() > MAX_BULK_SESSION_CANDIDATES {
            return Err(AppError::Validation(format!(
                "一次最多撤销 {MAX_BULK_SESSION_CANDIDATES} 个登录设备"
            )));
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => {
                redis
                    .revoke_other_sessions_for_user(
                        tenant_id,
                        user_id,
                        current_sid,
                        candidates,
                        now,
                    )
                    .await
            }
            None => {
                self.memory
                    .revoke_other_sessions_for_user(tenant_id, user_id, current_sid, now)
            }
        }
    }

    /// 返回会话当前的活跃身份；已撤销和已过期会话不会暴露身份信息。
    pub async fn identity(&self, sid: &str) -> AppResult<Option<RefreshSessionIdentity>> {
        if sid.is_empty() {
            return Ok(None);
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.identity(sid, now).await,
            None => self.memory.identity(sid, now),
        }
    }

    /// 在一次 Redis 乐观事务或一次本地读锁中校验会话及其完整身份。
    pub async fn is_active_for_identity(
        &self,
        sid: &str,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<bool> {
        if sid.is_empty() || tenant_id.is_empty() {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => {
                redis
                    .is_active_for_identity(sid, tenant_id, user_id, now)
                    .await
            }
            None => self
                .memory
                .is_active_for_identity(sid, tenant_id, user_id, now),
        }
    }

    /// 从新索引读取指定租户用户的活跃会话 SID。
    ///
    /// 升级前会话尚未写入该索引，上层在兼容期仍需合并旧在线会话索引并逐个调用 `identity` 校验。
    pub async fn session_sids_for_user(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<Vec<String>> {
        if tenant_id.is_empty() {
            return Ok(Vec::new());
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.session_sids_for_user(tenant_id, user_id, now).await,
            None => self.memory.session_sids_for_user(tenant_id, user_id, now),
        }
    }

    /// 从新索引读取指定租户的活跃会话 SID。
    pub async fn session_sids_for_tenant(&self, tenant_id: &str) -> AppResult<Vec<String>> {
        if tenant_id.is_empty() {
            return Ok(Vec::new());
        }
        let now = chrono::Utc::now().timestamp();
        match &self.redis {
            Some(redis) => redis.session_sids_for_tenant(tenant_id, now).await,
            None => self.memory.session_sids_for_tenant(tenant_id, now),
        }
    }

    pub async fn is_active(&self, sid: &str) -> AppResult<bool> {
        Ok(self.identity(sid).await?.is_some())
    }
}
