//! Redis 与进程内固定窗口限流实现。

use std::{
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use redis::AsyncCommands;

use crate::RedisClient;

const RATE_LIMIT_KEY_PREFIX: &str = "ryframe:v0.5:rate-limit:";
#[derive(Debug)]
struct WindowBucket {
    count: u32,
    reset_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after_secs: u64,
}

/// 固定窗口限流器的只读快照。
///
/// 快照不会创建桶或递增计数，仅用于展示当前窗口；`remaining_secs = 0` 表示当前没有活跃窗口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitSnapshot {
    pub key: String,
    pub current: u64,
    pub limit: u32,
    pub remaining_secs: u64,
}

#[derive(Clone)]
pub struct RateLimiter {
    mode: RateLimiterMode,
}

#[derive(Clone)]
enum RateLimiterMode {
    Redis {
        client: Box<RedisClient>,
        default_capacity: u32,
        default_window_secs: u64,
    },
    InMemory {
        inner: Arc<RateLimiterInner>,
    },
}

struct RateLimiterInner {
    buckets: DashMap<String, WindowBucket>,
    default_capacity: u32,
    default_window_secs: u64,
}

impl RateLimiter {
    pub fn new_redis(client: RedisClient, capacity: u32, window_secs: u64) -> Self {
        Self {
            mode: RateLimiterMode::Redis {
                client: Box::new(client),
                default_capacity: capacity.max(1),
                default_window_secs: window_secs.max(1),
            },
        }
    }

    /// 创建仅用于开发与隔离环境的内存固定窗口限流器。
    pub fn new_in_memory(capacity: u32, window_secs: u64) -> Self {
        Self {
            mode: RateLimiterMode::InMemory {
                inner: Arc::new(RateLimiterInner {
                    buckets: DashMap::new(),
                    default_capacity: capacity.max(1),
                    default_window_secs: window_secs.max(1),
                }),
            },
        }
    }

    pub async fn acquire(
        &self,
        key: &str,
        window_secs: u64,
        limit: u32,
    ) -> Result<RateLimitDecision, String> {
        let window_secs = window_secs.max(1);
        let limit = limit.max(1);

        match &self.mode {
            RateLimiterMode::Redis { client, .. } => {
                let redis_key = client.scoped_key(&format!("{RATE_LIMIT_KEY_PREFIX}{key}"));
                let watched = [redis_key.clone()];
                match client
                    .transaction(&watched, move |mut connection, mut transaction| {
                        let redis_key = redis_key.clone();
                        async move {
                            let current: Option<u64> = connection.get(&redis_key).await?;
                            let ttl: i64 = connection.ttl(&redis_key).await?;
                            let count = current.unwrap_or(0).saturating_add(1);
                            transaction.incr(&redis_key, 1_u8).ignore();
                            if current.is_none() || ttl < 0 {
                                transaction
                                    .expire(&redis_key, redis_ttl_secs(window_secs))
                                    .ignore();
                            }
                            let committed: Option<()> =
                                transaction.query_async(&mut connection).await?;
                            Ok(committed.map(|()| count <= u64::from(limit)))
                        }
                    })
                    .await
                {
                    Ok(allowed) => Ok(RateLimitDecision {
                        allowed,
                        retry_after_secs: window_secs,
                    }),
                    Err(error) => Err(format!("Redis rate-limit operation failed: {error}")),
                }
            }
            RateLimiterMode::InMemory { inner } => {
                let now = Instant::now();
                let reset_at = now + Duration::from_secs(window_secs);
                let mut bucket = inner
                    .buckets
                    .entry(key.to_string())
                    .or_insert(WindowBucket { count: 0, reset_at });
                if bucket.reset_at <= now {
                    bucket.count = 0;
                    bucket.reset_at = reset_at;
                }
                bucket.count = bucket.count.saturating_add(1);
                Ok(RateLimitDecision {
                    allowed: bucket.count <= limit,
                    retry_after_secs: bucket
                        .reset_at
                        .saturating_duration_since(now)
                        .as_secs()
                        .max(1),
                })
            }
        }
    }

    pub async fn try_acquire(&self, key: &str) -> bool {
        let (capacity, window_secs) = self.default_rule();
        self.acquire(key, window_secs, capacity)
            .await
            .is_ok_and(|decision| decision.allowed)
    }

    /// 批量读取固定窗口状态，不改变任何限流计数。
    ///
    /// Redis 模式使用单次原子管道读取全部计数和 TTL；内存模式直接读取已有桶。
    pub async fn snapshot_many(
        &self,
        keys: &[String],
        limit: u32,
    ) -> Result<Vec<RateLimitSnapshot>, String> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        match &self.mode {
            RateLimiterMode::Redis { client, .. } => {
                let redis_keys = keys
                    .iter()
                    .map(|key| client.scoped_key(&format!("{RATE_LIMIT_KEY_PREFIX}{key}")))
                    .collect::<Vec<_>>();
                let watched = redis_keys.clone();
                let values = client
                    .transaction(&watched, move |mut connection, mut transaction| {
                        let redis_keys = redis_keys.clone();
                        async move {
                            for redis_key in &redis_keys {
                                transaction.get(redis_key).ttl(redis_key);
                            }
                            transaction.query_async(&mut connection).await
                        }
                    })
                    .await
                    .map_err(|error| format!("Redis rate-limit snapshot failed: {error}"))?;
                parse_redis_snapshots(keys, limit, values)
            }
            RateLimiterMode::InMemory { inner } => {
                let now = Instant::now();
                Ok(keys
                    .iter()
                    .map(|key| {
                        let (current, remaining_secs) = inner
                            .buckets
                            .get(key)
                            .filter(|bucket| bucket.reset_at > now)
                            .map(|bucket| {
                                (
                                    u64::from(bucket.count),
                                    bucket
                                        .reset_at
                                        .saturating_duration_since(now)
                                        .as_secs()
                                        .max(1),
                                )
                            })
                            .unwrap_or((0, 0));
                        RateLimitSnapshot {
                            key: key.clone(),
                            current,
                            limit,
                            remaining_secs,
                        }
                    })
                    .collect())
            }
        }
    }

    pub fn spawn_gc(self: &Arc<Self>) {
        if let RateLimiterMode::InMemory { inner } = &self.mode {
            let inner = Arc::clone(inner);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let now = Instant::now();
                    inner.buckets.retain(|_, bucket| bucket.reset_at > now);
                }
            });
        }
    }

    fn default_rule(&self) -> (u32, u64) {
        match &self.mode {
            RateLimiterMode::Redis {
                default_capacity,
                default_window_secs,
                ..
            } => (*default_capacity, *default_window_secs),
            RateLimiterMode::InMemory { inner } => {
                (inner.default_capacity, inner.default_window_secs)
            }
        }
    }

    pub fn user_key(user_id: &str) -> String {
        format!("user:{user_id}")
    }

    pub fn tenant_user_key(tenant_id: &str, user_id: &str) -> String {
        format!("tenant_user:{tenant_id}:{user_id}")
    }

    pub fn tenant_key(tenant_id: &str) -> String {
        format!("tenant:{tenant_id}")
    }

    pub fn api_key(path: &str) -> String {
        format!("api:{path}")
    }

    pub fn api_client_key(path: &str, client_ip: IpAddr) -> String {
        format!("api:{path}:ip:{client_ip}")
    }

    pub fn user_api_key(user_id: &str, path: &str) -> String {
        format!("user_api:{user_id}:{path}")
    }
}

fn parse_redis_snapshots(
    keys: &[String],
    limit: u32,
    values: Vec<redis::Value>,
) -> Result<Vec<RateLimitSnapshot>, String> {
    if values.len() != keys.len().saturating_mul(2) {
        return Err("Redis rate-limit snapshot returned an invalid item count".into());
    }
    keys.iter()
        .enumerate()
        .map(|(index, key)| {
            let current = redis_snapshot_integer(&values[index * 2])?;
            let remaining_secs = redis_snapshot_integer(&values[index * 2 + 1])?;
            Ok(RateLimitSnapshot {
                key: key.clone(),
                current,
                limit,
                remaining_secs,
            })
        })
        .collect()
}

fn redis_snapshot_integer(value: &redis::Value) -> Result<u64, String> {
    match value {
        // 不存在的窗口：`GET` 返回 nil、`TTL` 返回负整数，统一按“无活跃窗口”读取。
        redis::Value::Nil => Ok(0),
        redis::Value::Int(value) if *value < 0 => Ok(0),
        // `INCR` 写出的整数在部分驱动路径下以整数回复返回，`GET` 路径则
        // 以 bulk string 返回；两种类型都应解析，避免读取方与写入方协议不一致。
        redis::Value::Int(value) => u64::try_from(*value)
            .map_err(|_| "Redis rate-limit snapshot returned a negative value".to_owned()),
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| format!("unexpected Redis rate-limit snapshot item: {value:?}")),
        _ => Err(format!(
            "unexpected Redis rate-limit snapshot item: {value:?}"
        )),
    }
}

fn redis_ttl_secs(ttl_secs: u64) -> i64 {
    ttl_secs.min(i64::MAX as u64) as i64
}
