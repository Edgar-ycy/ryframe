use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use ryframe_common::{AppError, AppResult, CAPTCHA_KEY_PREFIX};
use ryframe_core::RedisClient;
use tokio::sync::Mutex;

/// 验证码条目（含过期时间，仅内存模式使用）
pub struct CaptchaEntry {
    pub answer: String,
    pub created_at: Instant,
}

/// 验证码存储（支持 Redis / 内存双模式）
///
/// - Redis 模式：使用 Redis SET EX，自动过期，无需后台 GC
/// - 内存模式：HashMap + TTL，后台每 2 分钟自动清理
#[derive(Clone)]
pub enum CaptchaStore {
    /// Redis 存储（生产推荐）
    Redis {
        client: Box<RedisClient>,
        ttl_secs: u64,
    },
    /// 内存存储（开发/降级模式）
    InMemory {
        inner: Arc<Mutex<HashMap<String, CaptchaEntry>>>,
        ttl: Duration,
    },
}

impl CaptchaStore {
    /// 创建 Redis 模式的验证码存储
    pub fn new_redis(client: RedisClient, ttl_secs: u64) -> Self {
        Self::Redis {
            client: Box::new(client),
            ttl_secs,
        }
    }

    /// 创建内存模式的验证码存储，指定 TTL（秒）
    pub fn new_in_memory(ttl_secs: u64) -> Self {
        Self::InMemory {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// 存储验证码
    pub async fn set(&self, id: String, answer: String) -> AppResult<()> {
        match self {
            Self::Redis { client, ttl_secs } => {
                let key = format!("{}{}", CAPTCHA_KEY_PREFIX, id);
                client
                    .set_ex(&key, &answer, *ttl_secs)
                    .await
                    .map_err(|error| {
                        tracing::error!(%error, "Redis SET 验证码失败");
                        AppError::ServiceUnavailable("验证码服务暂不可用".into())
                    })?;
            }
            Self::InMemory { inner, ttl: _ } => {
                let mut store = inner.lock().await;
                store.insert(
                    id,
                    CaptchaEntry {
                        answer,
                        created_at: Instant::now(),
                    },
                );
            }
        }
        Ok(())
    }

    /// 校验验证码（一次性使用，校验后自动删除）
    pub async fn verify(&self, id: &str, code: &str) -> AppResult<bool> {
        match self {
            Self::Redis { client, .. } => {
                let key = format!("{}{}", CAPTCHA_KEY_PREFIX, id);
                match client.get_and_del(&key).await {
                    Ok(Some(stored)) => Ok(stored.eq_ignore_ascii_case(code)),
                    Ok(None) => Ok(false),
                    Err(error) => {
                        tracing::error!(%error, "Redis GETDEL 验证码失败");
                        Err(AppError::ServiceUnavailable("验证码服务暂不可用".into()))
                    }
                }
            }
            Self::InMemory { inner, ttl } => {
                let mut store = inner.lock().await;
                if let Some(entry) = store.remove(id) {
                    if entry.created_at.elapsed() > *ttl {
                        return Ok(false);
                    }
                    Ok(entry.answer.eq_ignore_ascii_case(code))
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// 启动后台定时清理任务（仅内存模式有效）
    pub fn spawn_gc(&self) {
        if let Self::InMemory { inner, ttl } = self {
            let inner = inner.clone();
            let ttl = *ttl;
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(120));
                loop {
                    interval.tick().await;
                    let mut store = inner.lock().await;
                    store.retain(|_, entry| entry.created_at.elapsed() <= ttl);
                }
            });
        }
        // Redis 模式由 Redis 自身 TTL 机制管理，无需后台 GC
    }
}
