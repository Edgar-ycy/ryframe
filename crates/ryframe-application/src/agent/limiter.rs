use redis::AsyncCommands;
use ryframe_adapters::RedisClient;
use ryframe_kernel::{AppError, AppResult};
use sha2::{Digest, Sha256};

const WINDOW_SECS: u64 = 60;
const KEY_PREFIX: &str = "ryframe:v0.9:agent-limit:";

#[derive(Clone)]
pub(super) struct AgentLimiter {
    redis: RedisClient,
}

pub(super) struct AgentLimitInput<'a> {
    pub ip: &'a str,
    pub tenant_id: &'a str,
    pub tenant_limit: i32,
    pub account_id: i64,
    pub account_limit: i32,
    pub credential_id: i64,
    pub represented_user_id: Option<i64>,
    pub capability_key: &'static str,
    pub capability_cost: u32,
    pub default_limit: u32,
    pub concurrency_limit: u32,
    pub concurrency_ttl_ms: u64,
    pub owner: &'a str,
}

pub(super) struct AgentConcurrencyLease {
    limiter: AgentLimiter,
    key: String,
    owner: String,
}

impl AgentLimiter {
    pub fn new(redis: RedisClient) -> Self {
        Self { redis }
    }

    /// 在读取公开 Key ID 前先执行独立的 IP 防刷；正式身份验证后仍会原子执行完整七维决策。
    pub async fn guard_pre_auth_ip(&self, ip: &str, limit: u32) -> AppResult<()> {
        let key = self.redis.scoped_key(&digest_key("pre-auth-ip", ip));
        let watched = [key.clone()];
        let code: i64 = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let key = key.clone();
                async move {
                    let current: Option<u64> = connection.get(&key).await?;
                    let ttl: i64 = connection.ttl(&key).await?;
                    let next = current.unwrap_or(0).saturating_add(1);
                    transaction.incr(&key, 1_u8).ignore();
                    if current.is_none() || ttl < 0 {
                        transaction
                            .expire(&key, redis_ttl_secs(WINDOW_SECS))
                            .ignore();
                    }
                    let retry_after = if ttl > 0 { ttl as u64 } else { WINDOW_SECS };
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    let allowed = next <= u64::from(limit.max(1));
                    Ok(committed.map(|()| {
                        if allowed {
                            1
                        } else {
                            -(retry_after as i64).max(1)
                        }
                    }))
                }
            })
            .await
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务暂不可用".into()))?;
        if code == 1 {
            Ok(())
        } else {
            Err(AppError::RateLimited(
                "Agent 请求过于频繁".into(),
                code.unsigned_abs().max(1),
            ))
        }
    }

    pub async fn acquire(&self, input: AgentLimitInput<'_>) -> AppResult<AgentConcurrencyLease> {
        let mut dimensions = Vec::<(String, u32, u32)>::new();
        // 预认证 IP 桶已经承担独立 IP 维度；完整决策仍会读取该桶，避免重复计数。
        dimensions.push((
            self.redis.scoped_key(&digest_key("pre-auth-ip", input.ip)),
            input.default_limit,
            0,
        ));
        if let Ok(limit) = u32::try_from(input.tenant_limit)
            && limit > 0
        {
            dimensions.push((
                self.redis
                    .scoped_key(&digest_key("tenant", input.tenant_id)),
                limit,
                1,
            ));
        }
        let account_limit = u32::try_from(input.account_limit)
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or(input.default_limit);
        dimensions.push((
            self.redis
                .scoped_key(&digest_key("account", &input.account_id.to_string())),
            account_limit,
            1,
        ));
        dimensions.push((
            self.redis
                .scoped_key(&digest_key("credential", &input.credential_id.to_string())),
            input.default_limit,
            1,
        ));
        if let Some(user_id) = input.represented_user_id {
            dimensions.push((
                self.redis
                    .scoped_key(&digest_key("delegated-user", &user_id.to_string())),
                input.default_limit,
                1,
            ));
        }
        dimensions.push((
            self.redis.scoped_key(&digest_key(
                "capability",
                &format!("{}:{}", input.account_id, input.capability_key),
            )),
            account_limit,
            input.capability_cost.max(1),
        ));
        let concurrency_key = self
            .redis
            .scoped_key(&digest_key("concurrency", &input.account_id.to_string()));
        let mut keys = dimensions
            .iter()
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>();
        keys.push(concurrency_key.clone());
        let watched = keys.clone();
        let owner = input.owner.to_owned();
        let concurrency_limit = input.concurrency_limit.max(1);
        let concurrency_ttl_ms = input.concurrency_ttl_ms.max(1_000);
        let transaction_concurrency_key = concurrency_key.clone();
        let code: i64 = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let dimensions = dimensions.clone();
                let concurrency_key = transaction_concurrency_key.clone();
                let owner = owner.clone();
                async move {
                    let mut retry_after = 1_u64;
                    for (key, limit, cost) in &dimensions {
                        let current: Option<u64> = connection.get(key).await?;
                        let ttl: i64 = connection.ttl(key).await?;
                        if current.unwrap_or(0).saturating_add(u64::from(*cost)) > u64::from(*limit)
                        {
                            retry_after = retry_after.max(u64::try_from(ttl).unwrap_or(1));
                            return Ok(Some(-(retry_after as i64).max(1)));
                        }
                    }
                    let (seconds, microseconds): (i64, i64) =
                        redis::cmd("TIME").query_async(&mut connection).await?;
                    let now_ms = seconds
                        .saturating_mul(1_000)
                        .saturating_add(microseconds.saturating_div(1_000));
                    let active_count: usize = redis::cmd("ZCOUNT")
                        .arg(&concurrency_key)
                        .arg(format!("({now_ms}"))
                        .arg("+inf")
                        .query_async(&mut connection)
                        .await?;
                    if active_count >= concurrency_limit as usize {
                        let oldest: Vec<(String, f64)> = redis::cmd("ZRANGEBYSCORE")
                            .arg(&concurrency_key)
                            .arg(format!("({now_ms}"))
                            .arg("+inf")
                            .arg("WITHSCORES")
                            .arg("LIMIT")
                            .arg(0)
                            .arg(1)
                            .query_async(&mut connection)
                            .await?;
                        let retry_after = oldest
                            .first()
                            .map(|(_, expires_at)| {
                                (((*expires_at as i64).saturating_sub(now_ms) + 999) / 1_000).max(1)
                                    as u64
                            })
                            .unwrap_or(1);
                        return Ok(Some(-(retry_after as i64).max(1)));
                    }
                    for (key, _, cost) in &dimensions {
                        if *cost == 0 {
                            continue;
                        }
                        let current: Option<u64> = connection.get(key).await?;
                        let ttl: i64 = connection.ttl(key).await?;
                        transaction.incr(key, *cost).ignore();
                        if current.is_none() || ttl < 0 {
                            transaction
                                .expire(key, redis_ttl_secs(WINDOW_SECS))
                                .ignore();
                        }
                    }
                    transaction
                        .cmd("ZREMRANGEBYSCORE")
                        .arg(&concurrency_key)
                        .arg("-inf")
                        .arg(now_ms)
                        .ignore()
                        .zadd(
                            &concurrency_key,
                            owner,
                            now_ms.saturating_add(concurrency_ttl_ms as i64),
                        )
                        .ignore();
                    let existing_ttl: i64 = connection.pttl(&concurrency_key).await?;
                    if existing_ttl < concurrency_ttl_ms as i64 {
                        transaction
                            .pexpire(&concurrency_key, redis_ttl_ms(concurrency_ttl_ms))
                            .ignore();
                    }
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| 1_i64))
                }
            })
            .await
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务暂不可用".into()))?;
        if code != 1 {
            return Err(AppError::RateLimited(
                "Agent 请求过于频繁".into(),
                code.unsigned_abs().max(1),
            ));
        }
        Ok(AgentConcurrencyLease {
            limiter: self.clone(),
            key: concurrency_key,
            owner: input.owner.to_owned(),
        })
    }
}

impl AgentConcurrencyLease {
    pub async fn release(self) {
        let mut connection = self.limiter.redis.conn().clone();
        let result: redis::RedisResult<usize> = connection.zrem(&self.key, &self.owner).await;
        if result.is_err() {
            tracing::warn!("Agent 并发租约释放失败，将由 TTL 自动回收");
        }
    }
}

fn digest_key(dimension: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(dimension.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    format!("{KEY_PREFIX}{dimension}:{}", hex::encode(digest.finalize()))
}

fn redis_ttl_secs(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn redis_ttl_ms(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
