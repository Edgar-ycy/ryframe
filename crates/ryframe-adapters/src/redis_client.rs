//! Redis 客户端封装
//!
//! 提供异步 Redis 连接管理器和常用操作封装。
//! 当 Redis 未配置时，调用方应回退到内存存储。

use std::{future::Future, sync::Arc, time::Duration};

use redis::{
    AsyncCommands, FromRedisValue, Pipeline,
    aio::{ConnectionManager, ConnectionManagerConfig, MultiplexedConnection},
};
use ryframe_config::RedisConfig;
use tracing::Instrument;

const SCAN_BATCH_SIZE: usize = 256;
/// Redis 客户端 span 使用的固定操作集合，禁止将键和参数内容作为属性。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RedisOperation {
    Connect,
    Subscribe,
    Set,
    SetEx,
    Get,
    Mget,
    Del,
    Publish,
    GetAndDel,
    Exists,
    Ttl,
    Ping,
    ConfigGet,
    Scan,
    DeleteByPattern,
    Hset,
    Hgetall,
    Hdel,
    Expire,
    Incr,
    Decr,
    Transaction,
}

impl RedisOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "CONNECT",
            Self::Subscribe => "SUBSCRIBE",
            Self::Set => "SET",
            Self::SetEx => "SET_EX",
            Self::Get => "GET",
            Self::Mget => "MGET",
            Self::Del => "DEL",
            Self::Publish => "PUBLISH",
            Self::GetAndDel => "GET_AND_DEL",
            Self::Exists => "EXISTS",
            Self::Ttl => "TTL",
            Self::Ping => "PING",
            Self::ConfigGet => "CONFIG_GET",
            Self::Scan => "SCAN",
            Self::DeleteByPattern => "DELETE_BY_PATTERN",
            Self::Hset => "HSET",
            Self::Hgetall => "HGETALL",
            Self::Hdel => "HDEL",
            Self::Expire => "EXPIRE",
            Self::Incr => "INCR",
            Self::Decr => "DECR",
            Self::Transaction => "TRANSACTION",
        }
    }
}

fn redis_operation_span(operation: RedisOperation) -> tracing::Span {
    tracing::info_span!(
        "redis.command",
        otel.name = operation.as_str(),
        otel.kind = "client",
        db.system.name = "redis",
        db.operation.name = operation.as_str(),
        redis.result = tracing::field::Empty,
    )
}

async fn trace_redis_operation<T>(
    operation: RedisOperation,
    future: impl Future<Output = Result<T, redis::RedisError>>,
) -> Result<T, redis::RedisError> {
    let span = redis_operation_span(operation);
    let result = future.instrument(span.clone()).await;
    span.record("redis.result", redis_result_label(&result));
    result
}

fn redis_result_label<T>(result: &Result<T, redis::RedisError>) -> &'static str {
    if result.is_ok() { "success" } else { "error" }
}

fn prepare_mget_command(keys: &[String]) -> redis::Cmd {
    let mut command = redis::cmd("MGET");
    for key in keys {
        command.arg(key);
    }
    command
}

/// Redis 客户端封装
///
/// 内部使用 `ConnectionManager`，自动处理重连和连接池管理。
#[derive(Clone)]
pub struct RedisClient {
    client: redis::Client,
    conn: ConnectionManager,
    timeout: Duration,
    namespace: RedisNamespace,
}

/// 可安全传入 Redis 事务闭包的不可变命名空间生成器。
#[derive(Clone, Debug)]
pub struct RedisNamespace(Arc<str>);

impl RedisNamespace {
    pub fn key(&self, logical: &str) -> String {
        if logical.starts_with(self.0.as_ref()) {
            return logical.to_owned();
        }
        let suffix = logical.strip_prefix("ryframe:").unwrap_or(logical);
        format!("{}{suffix}", self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RedisClient {
    /// 从配置创建 Redis 客户端
    ///
    /// # 错误
    /// 连接超时或 Redis 不可达时返回错误
    pub async fn connect(config: &RedisConfig) -> Result<Self, redis::RedisError> {
        let (client, conn) = trace_redis_operation(RedisOperation::Connect, async {
            let client = build_client(config).await?;

            // 带超时的连接。
            let timeout = Duration::from_secs(config.timeout_secs.max(1));
            let manager_config = ConnectionManagerConfig::new()
                .set_connection_timeout(Some(timeout))
                .set_response_timeout(Some(timeout));
            let conn = tokio::time::timeout(
                timeout,
                ConnectionManager::new_with_config(client.clone(), manager_config),
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
            Ok((client, conn))
        })
        .await?;

        tracing::info!("Redis 连接成功: {}:{}", config.host, config.port);
        Ok(Self {
            client,
            conn,
            timeout: Duration::from_secs(config.timeout_secs.max(1)),
            namespace: RedisNamespace(Arc::from(config.namespace())),
        })
    }

    /// 将逻辑键转换为当前部署环境唯一的物理键。
    ///
    /// 已生成的当前 scope 物理键保持不变，方便事务同时用于 WATCH 和命令参数；任何其他
    /// `ryframe:` 前缀都只会成为当前 namespace 内的后缀，不能逃逸到其他环境。
    pub fn scoped_key(&self, key: &str) -> String {
        self.scoped_name(key)
    }

    /// 将逻辑频道转换为当前部署环境唯一的物理频道。
    pub fn scoped_channel(&self, channel: &str) -> String {
        self.scoped_name(channel)
    }

    pub fn namespace(&self) -> &str {
        self.namespace.as_str()
    }

    fn scoped_name(&self, logical: &str) -> String {
        self.namespace.key(logical)
    }

    pub fn keyspace(&self) -> RedisNamespace {
        self.namespace.clone()
    }

    pub fn ownership_marker_key(&self) -> String {
        self.scoped_key(".ryframe-owner")
    }

    /// 在当前 scope 内原子声明所有权；已有标记只允许与期望值完全一致。
    pub async fn ensure_scope_ownership(&self, expected: &str) -> Result<(), redis::RedisError> {
        let key = self.ownership_marker_key();
        let mut connection = self.conn.clone();
        let _: Option<String> = trace_redis_operation(
            RedisOperation::Set,
            redis::cmd("SET")
                .arg(&key)
                .arg(expected)
                .arg("NX")
                .query_async(&mut connection),
        )
        .await?;
        self.verify_scope_ownership(expected).await
    }

    /// 只读校验 Redis namespace 的所有权标记。
    pub async fn verify_scope_ownership(&self, expected: &str) -> Result<(), redis::RedisError> {
        let actual = self.get(self.ownership_marker_key()).await?;
        if actual.as_deref() != Some(expected) {
            return Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "Redis scope ownership marker mismatch",
                format!("namespace={}", self.namespace()),
            )));
        }
        Ok(())
    }

    /// 获取底层连接管理器（用于高级操作）
    pub fn conn(&self) -> &ConnectionManager {
        &self.conn
    }

    /// 建立本次操作独占的多路复用连接。
    ///
    /// 需要 `WATCH/MULTI/EXEC` 的调用必须使用独占连接，避免多个乐观事务在共享连接上交错。
    async fn dedicated_connection(&self) -> Result<MultiplexedConnection, redis::RedisError> {
        tokio::time::timeout(self.timeout, self.client.get_multiplexed_async_connection())
            .await
            .map_err(|_| redis_timeout_error("Redis 独占连接超时"))?
    }

    /// 在独占连接上执行 Redis 乐观事务，检测到并发修改时自动重试。
    ///
    /// 闭包可能执行多次，闭包内只能进行可重复的 Redis 读取并构造事务命令，不能产生外部副作用。
    pub async fn transaction<K, T, F, Fut>(
        &self,
        keys: &[K],
        operation: F,
    ) -> Result<T, redis::RedisError>
    where
        K: AsRef<str>,
        T: FromRedisValue,
        F: FnMut(MultiplexedConnection, Pipeline) -> Fut,
        Fut: Future<Output = Result<Option<T>, redis::RedisError>>,
    {
        let connection = self.dedicated_connection().await?;
        let keys = keys
            .iter()
            .map(|key| self.scoped_key(key.as_ref()))
            .collect::<Vec<_>>();
        trace_redis_operation(RedisOperation::Transaction, async {
            tokio::time::timeout(
                self.timeout,
                redis::aio::transaction_async(connection, &keys, operation),
            )
            .await
            .map_err(|_| redis_timeout_error("Redis 事务超时"))?
        })
        .await
    }

    /// 建立一个专用的 Pub/Sub 订阅连接并订阅指定频道。
    ///
    /// 该连接不能与普通命令复用，调用方应在连接中断后自行按退避策略重建。
    pub async fn subscribe(&self, channel: &str) -> Result<redis::aio::PubSub, redis::RedisError> {
        let channel = self.scoped_channel(channel);
        trace_redis_operation(RedisOperation::Subscribe, async {
            let mut subscription = self.client.get_async_pubsub().await?;
            subscription.subscribe(channel).await?;
            Ok(subscription)
        })
        .await
    }

    /// 建立专用 Pub/Sub 连接并订阅多个频道。
    pub async fn subscribe_many(
        &self,
        channels: &[&str],
    ) -> Result<redis::aio::PubSub, redis::RedisError> {
        let channels = channels
            .iter()
            .map(|channel| self.scoped_channel(channel))
            .collect::<Vec<_>>();
        trace_redis_operation(RedisOperation::Subscribe, async {
            let mut subscription = self.client.get_async_pubsub().await?;
            for channel in channels {
                subscription.subscribe(channel).await?;
            }
            Ok(subscription)
        })
        .await
    }

    // ========== 便捷方法 ==========

    /// 执行 `SET key value`（无过期时间）。
    pub async fn set<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Set, conn.set(key, value.as_ref())).await
    }

    /// 执行 `SET key value EX seconds`（带过期时间）。
    pub async fn set_ex<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
        seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(
            RedisOperation::SetEx,
            conn.set_ex(key, value.as_ref(), seconds),
        )
        .await
    }

    /// 执行 `GET key`（不存在时返回 `None`）。
    pub async fn get<K: AsRef<str>>(&self, key: K) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Get, conn.get(key)).await
    }

    /// 对多个键执行 `MGET`。返回值与输入键一一对应，不存在或已过期的键为 `None`。
    pub async fn mget<K: AsRef<str>>(
        &self,
        keys: &[K],
    ) -> Result<Vec<Option<String>>, redis::RedisError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let keys = keys
            .iter()
            .map(|key| self.scoped_key(key.as_ref()))
            .collect::<Vec<_>>();
        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::Mget,
            prepare_mget_command(&keys).query_async(&mut conn),
        )
        .await
    }

    /// 执行 `DEL key`，删除键并返回删除数量。
    pub async fn del<K: AsRef<str>>(&self, key: K) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Del, conn.del(key)).await
    }

    /// 向 Redis Pub/Sub 频道发布文本负载，并返回已接收的订阅者数量。
    ///
    /// 发布只用于低延迟唤醒；需要可靠处理的业务事实仍必须先写入数据库。
    pub async fn publish<C: AsRef<str>, P: AsRef<str>>(
        &self,
        channel: C,
        payload: P,
    ) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let channel = self.scoped_channel(channel.as_ref());
        trace_redis_operation(
            RedisOperation::Publish,
            conn.publish(channel, payload.as_ref()),
        )
        .await
    }

    /// 原子获取并删除，用于验证码等一次性数据。
    pub async fn get_and_del<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(
            RedisOperation::GetAndDel,
            redis::cmd("GETDEL").arg(key).query_async(&mut conn),
        )
        .await
    }

    /// 执行 `EXISTS key`。
    pub async fn exists<K: AsRef<str>>(&self, key: K) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Exists, conn.exists(key)).await
    }

    /// 执行 `TTL key`（返回剩余秒数，-1 表示永不过期，-2 表示不存在）。
    pub async fn ttl<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Ttl, conn.ttl(key)).await
    }

    /// 执行 `PING`。
    pub async fn ping(&self) -> Result<String, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::Ping,
            redis::cmd("PING").query_async(&mut conn),
        )
        .await
    }

    /// 读取一项 Redis 服务端配置值。生产启动用它确保安全状态启用持久化和禁止淘汰策略。
    pub async fn config_get(&self, name: &str) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        let values: std::collections::HashMap<String, String> = trace_redis_operation(
            RedisOperation::ConfigGet,
            redis::cmd("CONFIG")
                .arg("GET")
                .arg(name)
                .query_async(&mut conn),
        )
        .await?;
        Ok(values.get(name).cloned())
    }

    /// 使用增量游标扫描匹配的键，避免 `KEYS` 阻塞 Redis。
    pub async fn scan_keys<K: AsRef<str>>(
        &self,
        pattern: K,
    ) -> Result<Vec<String>, redis::RedisError> {
        let pattern = self.scoped_key(pattern.as_ref());
        let mut conn = self.conn.clone();
        let mut cursor = 0_u64;
        let mut keys = Vec::new();

        loop {
            let (next_cursor, mut batch): (u64, Vec<String>) = trace_redis_operation(
                RedisOperation::Scan,
                redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(SCAN_BATCH_SIZE)
                    .query_async(&mut conn),
            )
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

    /// 逐步查找并删除所有匹配 `pattern` 的键。
    pub async fn delete_by_pattern<K: AsRef<str>>(
        &self,
        pattern: K,
    ) -> Result<u64, redis::RedisError> {
        trace_redis_operation(RedisOperation::DeleteByPattern, async {
            let keys = self.scan_keys(pattern).await?;
            let mut conn = self.conn.clone();
            let mut deleted = 0_u64;

            for batch in keys.chunks(SCAN_BATCH_SIZE) {
                let mut command = redis::cmd("UNLINK");
                for key in batch {
                    command.arg(key);
                }
                let removed: u64 =
                    trace_redis_operation(RedisOperation::Del, command.query_async(&mut conn))
                        .await?;
                deleted += removed;
            }
            Ok(deleted)
        })
        .await
    }

    /// 执行 `HSET key field value`。
    pub async fn hset<K: AsRef<str>, F: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        field: F,
        value: V,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(
            RedisOperation::Hset,
            conn.hset(key, field.as_ref(), value.as_ref()),
        )
        .await
    }

    /// 执行 `HGETALL key`（返回 `HashMap`）。
    pub async fn hgetall<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<std::collections::HashMap<String, String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Hgetall, conn.hgetall(key)).await
    }

    /// 执行 `HDEL key field`。
    pub async fn hdel<K: AsRef<str>, F: AsRef<str>>(
        &self,
        key: K,
        field: F,
    ) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Hdel, conn.hdel(key, field.as_ref())).await
    }

    /// 执行 `EXPIRE key seconds`。
    pub async fn expire<K: AsRef<str>>(
        &self,
        key: K,
        seconds: u64,
    ) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Expire, conn.expire(key, seconds as i64)).await
    }

    /// 执行 `INCR key`（原子递增）。
    pub async fn incr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Incr, conn.incr(key, 1)).await
    }

    /// 执行 `DECR key`（原子递减）。
    pub async fn decr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        let key = self.scoped_key(key.as_ref());
        trace_redis_operation(RedisOperation::Decr, conn.decr(key, 1)).await
    }
}

fn redis_timeout_error(message: &'static str) -> redis::RedisError {
    redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

async fn build_client(config: &RedisConfig) -> Result<redis::Client, redis::RedisError> {
    let url = config.connection_url();
    if !config.tls {
        return redis::Client::open(url);
    }

    let root_cert = read_optional_pem(config.tls_ca.as_deref()).await?;
    let client_tls = match (
        config.tls_client_cert.as_deref(),
        config.tls_client_key.as_deref(),
    ) {
        (Some(cert), Some(key)) => Some(redis::ClientTlsConfig {
            client_cert: read_pem(cert).await?,
            client_key: read_pem(key).await?,
        }),
        _ => None,
    };
    redis::Client::build_with_tls(
        url,
        redis::TlsCertificates {
            client_tls,
            root_cert,
        },
    )
}

async fn read_optional_pem(path: Option<&str>) -> Result<Option<Vec<u8>>, redis::RedisError> {
    match path.filter(|path| !path.trim().is_empty()) {
        Some(path) => read_pem(path).await.map(Some),
        None => Ok(None),
    }
}

async fn read_pem(path: &str) -> Result<Vec<u8>, redis::RedisError> {
    tokio::fs::read(path).await.map_err(|error| {
        redis::RedisError::from(std::io::Error::new(
            error.kind(),
            format!("unable to read Redis TLS file {path}: {error}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::RedisNamespace;

    #[test]
    fn namespace_is_idempotent_and_cannot_escape_to_another_scope() {
        let namespace = RedisNamespace(Arc::from("ryframe:{dev-a}:"));
        assert_eq!(namespace.key("jobs:wakeup"), "ryframe:{dev-a}:jobs:wakeup");
        assert_eq!(
            namespace.key("ryframe:v0.5:lock:tenant"),
            "ryframe:{dev-a}:v0.5:lock:tenant"
        );
        assert_eq!(
            namespace.key("ryframe:{dev-a}:jobs:wakeup"),
            "ryframe:{dev-a}:jobs:wakeup"
        );
        assert_eq!(
            namespace.key("ryframe:{other}:jobs:wakeup"),
            "ryframe:{dev-a}:{other}:jobs:wakeup"
        );
    }
}
