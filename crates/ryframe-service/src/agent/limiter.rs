use ryframe_core::RedisClient;
use ryframe_kernel::{AppError, AppResult};
use sha2::{Digest, Sha256};

const WINDOW_SECS: u64 = 60;
const KEY_PREFIX: &str = "ryframe:v0.9:agent-limit:";

const ACQUIRE_SCRIPT: &str = r#"
local dimension_count = tonumber(ARGV[1])
local window_secs = tonumber(ARGV[2])
local retry_after = 1
for index = 1, dimension_count do
    local limit = tonumber(ARGV[2 + (index - 1) * 2 + 1])
    local cost = tonumber(ARGV[2 + (index - 1) * 2 + 2])
    local current = tonumber(redis.call('GET', KEYS[index])) or 0
    if current + cost > limit then
        local ttl = redis.call('TTL', KEYS[index])
        if ttl > retry_after then retry_after = ttl end
        return {0, index, retry_after}
    end
end
local concurrency_key = KEYS[dimension_count + 1]
local tail = 2 + dimension_count * 2
local concurrency_limit = tonumber(ARGV[tail + 1])
local owner = ARGV[tail + 2]
local concurrency_ttl_ms = tonumber(ARGV[tail + 3])
local redis_time = redis.call('TIME')
local now_ms = tonumber(redis_time[1]) * 1000 + math.floor(tonumber(redis_time[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', concurrency_key, '-inf', now_ms)
if redis.call('ZCARD', concurrency_key) >= concurrency_limit then
    local oldest = redis.call('ZRANGE', concurrency_key, 0, 0, 'WITHSCORES')
    local retry = 1
    if oldest[2] then retry = math.max(1, math.ceil((tonumber(oldest[2]) - now_ms) / 1000)) end
    return {0, dimension_count + 1, retry}
end
for index = 1, dimension_count do
    local cost = tonumber(ARGV[2 + (index - 1) * 2 + 2])
    local count = redis.call('INCRBY', KEYS[index], cost)
    if count == cost or redis.call('TTL', KEYS[index]) < 0 then
        redis.call('EXPIRE', KEYS[index], window_secs)
    end
end
redis.call('ZADD', concurrency_key, now_ms + concurrency_ttl_ms, owner)
local existing_ttl = redis.call('PTTL', concurrency_key)
if existing_ttl < concurrency_ttl_ms then
    redis.call('PEXPIRE', concurrency_key, concurrency_ttl_ms)
end
return {1, 0, 0}
"#;

const RELEASE_SCRIPT: &str = r#"
if redis.call('ZSCORE', KEYS[1], ARGV[1]) then
    return redis.call('ZREM', KEYS[1], ARGV[1])
end
return 0
"#;

const PRE_AUTH_SCRIPT: &str = r#"
local count = redis.call('INCR', KEYS[1])
if count == 1 or redis.call('TTL', KEYS[1]) < 0 then
    redis.call('EXPIRE', KEYS[1], tonumber(ARGV[1]))
end
if count <= tonumber(ARGV[2]) then
    return {1, 0}
end
local ttl = redis.call('TTL', KEYS[1])
return {0, math.max(1, ttl)}
"#;

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
        let key = digest_key("pre-auth-ip", ip);
        let value = self
            .redis
            .eval_script(
                PRE_AUTH_SCRIPT,
                &[key],
                &[WINDOW_SECS.to_string(), limit.max(1).to_string()],
            )
            .await
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务暂不可用".into()))?;
        let values: Vec<i64> = redis::from_redis_value(value)
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务返回无效结果".into()))?;
        if values.len() != 2 {
            return Err(AppError::ServiceUnavailable(
                "Agent 限流服务返回无效结果".into(),
            ));
        }
        if values[0] == 1 {
            Ok(())
        } else {
            Err(AppError::RateLimited(
                "Agent 请求过于频繁".into(),
                u64::try_from(values[1]).unwrap_or(1).max(1),
            ))
        }
    }

    pub async fn acquire(&self, input: AgentLimitInput<'_>) -> AppResult<AgentConcurrencyLease> {
        let mut dimensions = Vec::<(String, u32, u32)>::new();
        // 预认证 IP 桶已经承担独立 IP 维度；完整决策仍在同一 Lua 中读取该桶，避免重复计数。
        dimensions.push((digest_key("pre-auth-ip", input.ip), input.default_limit, 0));
        if let Ok(limit) = u32::try_from(input.tenant_limit)
            && limit > 0
        {
            dimensions.push((digest_key("tenant", input.tenant_id), limit, 1));
        }
        let account_limit = u32::try_from(input.account_limit)
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or(input.default_limit);
        dimensions.push((
            digest_key("account", &input.account_id.to_string()),
            account_limit,
            1,
        ));
        dimensions.push((
            digest_key("credential", &input.credential_id.to_string()),
            input.default_limit,
            1,
        ));
        if let Some(user_id) = input.represented_user_id {
            dimensions.push((
                digest_key("delegated-user", &user_id.to_string()),
                input.default_limit,
                1,
            ));
        }
        dimensions.push((
            digest_key(
                "capability",
                &format!("{}:{}", input.account_id, input.capability_key),
            ),
            account_limit,
            input.capability_cost.max(1),
        ));
        let concurrency_key = digest_key("concurrency", &input.account_id.to_string());
        let mut keys = dimensions
            .iter()
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>();
        keys.push(concurrency_key.clone());
        let mut args = vec![dimensions.len().to_string(), WINDOW_SECS.to_string()];
        for (_, limit, cost) in &dimensions {
            args.push(limit.to_string());
            args.push(cost.to_string());
        }
        args.push(input.concurrency_limit.max(1).to_string());
        args.push(input.owner.to_owned());
        args.push(input.concurrency_ttl_ms.max(1_000).to_string());
        let value = self
            .redis
            .eval_script(ACQUIRE_SCRIPT, &keys, &args)
            .await
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务暂不可用".into()))?;
        let values: Vec<i64> = redis::from_redis_value(value)
            .map_err(|_| AppError::ServiceUnavailable("Agent 限流服务返回无效结果".into()))?;
        if values.len() != 3 {
            return Err(AppError::ServiceUnavailable(
                "Agent 限流服务返回无效结果".into(),
            ));
        }
        if values[0] != 1 {
            return Err(AppError::RateLimited(
                "Agent 请求过于频繁".into(),
                u64::try_from(values[2]).unwrap_or(1).max(1),
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
        if self
            .limiter
            .redis
            .eval_script_i64(RELEASE_SCRIPT, &[self.key], &[self.owner])
            .await
            .is_err()
        {
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
