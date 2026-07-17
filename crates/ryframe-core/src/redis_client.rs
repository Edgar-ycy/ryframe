//! Redis 客户端封装
//!
//! 提供异步 Redis 连接管理器和常用操作封装。
//! 当 Redis 未配置时，调用方应回退到内存存储。

use std::time::Duration;

use redis::{AsyncCommands, aio::ConnectionManager};
use ryframe_config::RedisConfig;

const SCAN_BATCH_SIZE: usize = 256;
const GET_AND_DEL_SCRIPT: &str = "local value = redis.call('GET', KEYS[1]); if value then redis.call('DEL', KEYS[1]); end; return value";

fn prepare_script_invocation<'script, K, V>(
    script: &'script redis::Script,
    keys: &[K],
    args: &[V],
) -> redis::ScriptInvocation<'script>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut invocation = script.prepare_invoke();
    for key in keys {
        invocation.key(key.as_ref());
    }
    for arg in args {
        invocation.arg(arg.as_ref());
    }
    invocation
}

/// Redis 客户端封装
///
/// 内部使用 `ConnectionManager`，自动处理重连和连接池管理。
#[derive(Clone)]
pub struct RedisClient {
    conn: ConnectionManager,
}

impl RedisClient {
    /// 从配置创建 Redis 客户端
    ///
    /// # Errors
    /// 连接超时或 Redis 不可达时返回错误
    pub async fn connect(config: &RedisConfig) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(config.connection_url())?;

        // 带超时的连接
        let conn = tokio::time::timeout(
            Duration::from_secs(config.timeout_secs),
            ConnectionManager::new(client),
        )
        .await
        .map_err(|_| {
            redis::RedisError::from(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Redis 连接超时 ({}s): {}:{}",
                    config.timeout_secs, config.host, config.port
                ),
            ))
        })??;

        tracing::info!("Redis 连接成功: {}:{}", config.host, config.port);
        Ok(Self { conn })
    }

    /// 获取底层连接管理器（用于高级操作）
    pub fn conn(&self) -> &ConnectionManager {
        &self.conn
    }

    // ========== 便捷方法 ==========

    /// SET key value（无过期）
    pub async fn set<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.set(key.as_ref(), value.as_ref()).await
    }

    /// SET key value EX seconds（带过期时间）
    pub async fn set_ex<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
        seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.set_ex(key.as_ref(), value.as_ref(), seconds).await
    }

    /// GET key（不存在返回 None）
    pub async fn get<K: AsRef<str>>(&self, key: K) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.get(key.as_ref()).await
    }

    /// DEL key（删除键，返回删除数量）
    pub async fn del<K: AsRef<str>>(&self, key: K) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.del(key.as_ref()).await
    }

    /// 原子获取并删除，用于验证码等一次性数据。
    pub async fn get_and_del<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        redis::cmd("EVAL")
            .arg(GET_AND_DEL_SCRIPT)
            .arg(1)
            .arg(key.as_ref())
            .query_async(&mut conn)
            .await
    }

    /// EXISTS key
    pub async fn exists<K: AsRef<str>>(&self, key: K) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.exists(key.as_ref()).await
    }

    /// TTL key（返回剩余秒数，-1=永不过期，-2=不存在）
    pub async fn ttl<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.ttl(key.as_ref()).await
    }

    /// PING
    pub async fn ping(&self) -> Result<String, redis::RedisError> {
        let mut conn = self.conn.clone();
        redis::cmd("PING").query_async(&mut conn).await
    }

    /// 使用增量游标扫描匹配的键，避免 `KEYS` 阻塞 Redis。
    pub async fn scan_keys<K: AsRef<str>>(
        &self,
        pattern: K,
    ) -> Result<Vec<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        let mut cursor = 0_u64;
        let mut keys = Vec::new();

        loop {
            let (next_cursor, mut batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern.as_ref())
                .arg("COUNT")
                .arg(SCAN_BATCH_SIZE)
                .query_async(&mut conn)
                .await?;
            keys.append(&mut batch);
            if next_cursor == 0 {
                break;
            }
            cursor = next_cursor;
        }

        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }

    /// Incrementally find and delete every key matching `pattern`.
    pub async fn delete_by_pattern<K: AsRef<str>>(
        &self,
        pattern: K,
    ) -> Result<u64, redis::RedisError> {
        let keys = self.scan_keys(pattern).await?;
        let mut conn = self.conn.clone();
        let mut deleted = 0_u64;

        for batch in keys.chunks(SCAN_BATCH_SIZE) {
            let mut command = redis::cmd("DEL");
            for key in batch {
                command.arg(key);
            }
            deleted += command.query_async::<u64>(&mut conn).await?;
        }
        Ok(deleted)
    }

    /// HSET key field value
    pub async fn hset<K: AsRef<str>, F: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        field: F,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hset(key.as_ref(), field.as_ref(), value.as_ref())
            .await
    }

    /// HGETALL key（返回 HashMap）
    pub async fn hgetall<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<std::collections::HashMap<String, String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hgetall(key.as_ref()).await
    }

    /// HDEL key field
    pub async fn hdel<K: AsRef<str>, F: AsRef<str>>(
        &self,
        key: K,
        field: F,
    ) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.hdel(key.as_ref(), field.as_ref()).await
    }

    /// EXPIRE key seconds
    pub async fn expire<K: AsRef<str>>(
        &self,
        key: K,
        seconds: u64,
    ) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.expire(key.as_ref(), seconds as i64).await
    }

    /// INCR key（原子递增）
    pub async fn incr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.incr(key.as_ref(), 1).await
    }

    /// DECR key（原子递减）
    pub async fn decr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        conn.decr(key.as_ref(), 1).await
    }

    /// EVAL Lua 脚本（用于原子操作，如滑动窗口限流）
    ///
    /// # Arguments
    /// - `script`: Lua 脚本内容
    /// - `keys`: KEYS 数组
    /// - `args`: ARGV 数组
    ///
    /// # Returns
    /// 脚本返回值（通常为整数或字符串）
    pub async fn eval_script<S: AsRef<str>, K: AsRef<str>, V: AsRef<str>>(
        &self,
        script: S,
        keys: &[K],
        args: &[V],
    ) -> Result<redis::Value, redis::RedisError> {
        let mut conn = self.conn.clone();
        let lua = redis::Script::new(script.as_ref());
        prepare_script_invocation(&lua, keys, args)
            .invoke_async(&mut conn)
            .await
    }
}

/// 根据配置创建 Redis 客户端
///
/// - 配置了 Redis → 尝试连接，成功返回 Some，失败返回 None（降级到内存模式）
/// - 未配置 Redis → 返回 None
pub async fn create_redis_client(config: &Option<RedisConfig>) -> Option<RedisClient> {
    let redis_config = config.as_ref()?;
    match RedisClient::connect(redis_config).await {
        Ok(client) => {
            // 验证连接
            match client.ping().await {
                Ok(_) => {
                    tracing::info!("Redis 服务就绪，启用 Redis 模式");
                    Some(client)
                }
                Err(e) => {
                    tracing::warn!("Redis PING 失败，降级到内存模式: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            tracing::warn!("Redis 连接失败，降级到内存模式: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use redis::{Arg, Cmd, Pipeline, RedisFuture, Value, aio::ConnectionLike};

    use super::prepare_script_invocation;

    #[derive(Default)]
    struct RecordingConnection {
        commands: Vec<Vec<Vec<u8>>>,
    }

    impl ConnectionLike for RecordingConnection {
        fn req_packed_command<'a>(&'a mut self, command: &'a Cmd) -> RedisFuture<'a, Value> {
            self.commands.push(
                command
                    .args_iter()
                    .filter_map(|arg| match arg {
                        Arg::Simple(value) => Some(value.to_vec()),
                        Arg::Cursor => None,
                    })
                    .collect(),
            );
            Box::pin(async { Ok(Value::Int(1)) })
        }

        fn req_packed_commands<'a>(
            &'a mut self,
            _pipeline: &'a Pipeline,
            _offset: usize,
            _count: usize,
        ) -> RedisFuture<'a, Vec<Value>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn get_db(&self) -> i64 {
            0
        }
    }

    #[tokio::test]
    async fn script_invocation_forwards_all_keys_and_arguments() {
        let script = redis::Script::new("return 1");
        let invocation = prepare_script_invocation(
            &script,
            &["rate-limit:a", "rate-limit:b"],
            &["100.25", "60", "10"],
        );
        let mut connection = RecordingConnection::default();

        let result: Value = invocation.invoke_async(&mut connection).await.unwrap();

        assert_eq!(result, Value::Int(1));
        assert_eq!(connection.commands.len(), 1);
        let command = &connection.commands[0];
        assert_eq!(command[0], b"EVALSHA");
        assert_eq!(command[2], b"2");
        assert_eq!(command[3], b"rate-limit:a");
        assert_eq!(command[4], b"rate-limit:b");
        assert_eq!(command[5], b"100.25");
        assert_eq!(command[6], b"60");
        assert_eq!(command[7], b"10");
    }
}
