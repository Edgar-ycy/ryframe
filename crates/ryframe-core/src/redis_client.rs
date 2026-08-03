//! Redis 客户端封装
//!
//! 提供异步 Redis 连接管理器和常用操作封装。
//! 当 Redis 未配置时，调用方应回退到内存存储。

use std::{future::Future, time::Duration};

use redis::{
    AsyncCommands,
    aio::{ConnectionManager, ConnectionManagerConfig},
};
use ryframe_config::RedisConfig;
use tracing::Instrument;

const SCAN_BATCH_SIZE: usize = 256;
const GET_AND_DEL_SCRIPT: &str = "local value = redis.call('GET', KEYS[1]); if value then redis.call('DEL', KEYS[1]); end; return value";

/// Redis 客户端 span 使用的固定操作集合，禁止将键、参数或脚本内容作为属性。
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
    Eval,
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
            Self::Eval => "EVAL",
        }
    }
}

fn redis_operation_span(operation: RedisOperation) -> tracing::Span {
    tracing::info_span!(
        "redis.command",
        otel.name = operation.as_str(),
        otel.kind = "client",
        db.system = "redis",
        db.operation = operation.as_str(),
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

fn prepare_mget_command<K: AsRef<str>>(keys: &[K]) -> redis::Cmd {
    let mut command = redis::cmd("MGET");
    for key in keys {
        command.arg(key.as_ref());
    }
    command
}

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
    client: redis::Client,
    conn: ConnectionManager,
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
        Ok(Self { client, conn })
    }

    /// 获取底层连接管理器（用于高级操作）
    pub fn conn(&self) -> &ConnectionManager {
        &self.conn
    }

    /// 建立一个专用的 Pub/Sub 订阅连接并订阅指定频道。
    ///
    /// 该连接不能与普通命令复用，调用方应在连接中断后自行按退避策略重建。
    pub async fn subscribe(&self, channel: &str) -> Result<redis::aio::PubSub, redis::RedisError> {
        trace_redis_operation(RedisOperation::Subscribe, async {
            let mut subscription = self.client.get_async_pubsub().await?;
            subscription.subscribe(channel).await?;
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
        trace_redis_operation(RedisOperation::Set, conn.set(key.as_ref(), value.as_ref())).await
    }

    /// 执行 `SET key value EX seconds`（带过期时间）。
    pub async fn set_ex<K: AsRef<str>, V: AsRef<str>>(
        &self,
        key: K,
        value: V,
        seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::SetEx,
            conn.set_ex(key.as_ref(), value.as_ref(), seconds),
        )
        .await
    }

    /// 执行 `GET key`（不存在时返回 `None`）。
    pub async fn get<K: AsRef<str>>(&self, key: K) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Get, conn.get(key.as_ref())).await
    }

    /// 对多个键执行 `MGET`。返回值与输入键一一对应，不存在或已过期的键为 `None`。
    pub async fn mget<K: AsRef<str>>(
        &self,
        keys: &[K],
    ) -> Result<Vec<Option<String>>, redis::RedisError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::Mget,
            prepare_mget_command(keys).query_async(&mut conn),
        )
        .await
    }

    /// 执行 `DEL key`，删除键并返回删除数量。
    pub async fn del<K: AsRef<str>>(&self, key: K) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Del, conn.del(key.as_ref())).await
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
        trace_redis_operation(
            RedisOperation::Publish,
            conn.publish(channel.as_ref(), payload.as_ref()),
        )
        .await
    }

    /// 原子获取并删除，用于验证码等一次性数据。
    pub async fn get_and_del<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::GetAndDel, async {
            redis::cmd("EVAL")
                .arg(GET_AND_DEL_SCRIPT)
                .arg(1)
                .arg(key.as_ref())
                .query_async(&mut conn)
                .await
        })
        .await
    }

    /// 执行 `EXISTS key`。
    pub async fn exists<K: AsRef<str>>(&self, key: K) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Exists, conn.exists(key.as_ref())).await
    }

    /// 执行 `TTL key`（返回剩余秒数，-1 表示永不过期，-2 表示不存在）。
    pub async fn ttl<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Ttl, conn.ttl(key.as_ref())).await
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
        let mut conn = self.conn.clone();
        let mut cursor = 0_u64;
        let mut keys = Vec::new();

        loop {
            let (next_cursor, mut batch): (u64, Vec<String>) = trace_redis_operation(
                RedisOperation::Scan,
                redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(pattern.as_ref())
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
                let mut command = redis::cmd("DEL");
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
        trace_redis_operation(
            RedisOperation::Hset,
            conn.hset(key.as_ref(), field.as_ref(), value.as_ref()),
        )
        .await
    }

    /// 执行 `HGETALL key`（返回 `HashMap`）。
    pub async fn hgetall<K: AsRef<str>>(
        &self,
        key: K,
    ) -> Result<std::collections::HashMap<String, String>, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Hgetall, conn.hgetall(key.as_ref())).await
    }

    /// 执行 `HDEL key field`。
    pub async fn hdel<K: AsRef<str>, F: AsRef<str>>(
        &self,
        key: K,
        field: F,
    ) -> Result<u64, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::Hdel,
            conn.hdel(key.as_ref(), field.as_ref()),
        )
        .await
    }

    /// 执行 `EXPIRE key seconds`。
    pub async fn expire<K: AsRef<str>>(
        &self,
        key: K,
        seconds: u64,
    ) -> Result<bool, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(
            RedisOperation::Expire,
            conn.expire(key.as_ref(), seconds as i64),
        )
        .await
    }

    /// 执行 `INCR key`（原子递增）。
    pub async fn incr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Incr, conn.incr(key.as_ref(), 1)).await
    }

    /// 执行 `DECR key`（原子递减）。
    pub async fn decr<K: AsRef<str>>(&self, key: K) -> Result<i64, redis::RedisError> {
        let mut conn = self.conn.clone();
        trace_redis_operation(RedisOperation::Decr, conn.decr(key.as_ref(), 1)).await
    }

    /// 执行 Lua 脚本（用于滑动窗口限流等原子操作）。
    ///
    /// # 参数
    /// - `script`: Lua 脚本内容
    /// - `keys`: KEYS 数组
    /// - `args`: ARGV 数组
    ///
    /// # 返回值
    /// 脚本返回值（通常为整数或字符串）
    pub async fn eval_script<S: AsRef<str>, K: AsRef<str>, V: AsRef<str>>(
        &self,
        script: S,
        keys: &[K],
        args: &[V],
    ) -> Result<redis::Value, redis::RedisError> {
        let mut conn = self.conn.clone();
        let lua = redis::Script::new(script.as_ref());
        trace_redis_operation(
            RedisOperation::Eval,
            prepare_script_invocation(&lua, keys, args).invoke_async(&mut conn),
        )
        .await
    }

    /// 执行返回整数状态码的 Lua 脚本。
    pub async fn eval_script_i64<S: AsRef<str>, K: AsRef<str>, V: AsRef<str>>(
        &self,
        script: S,
        keys: &[K],
        args: &[V],
    ) -> Result<i64, redis::RedisError> {
        let value = self.eval_script(script, keys, args).await?;
        redis::from_redis_value(value).map_err(|error| {
            redis::RedisError::from((
                redis::ErrorKind::Parse,
                "unable to parse Redis script response",
                error.to_string(),
            ))
        })
    }

    /// 执行返回可空字符串数组的 Lua 脚本。
    ///
    /// 该返回类型用于一次原子读取多个相互关联的缓存值；Lua 返回的 `false`
    /// 会被转换为 `None`，避免调用方依赖 Redis 协议值的内部表示。
    pub async fn eval_script_optional_strings<S: AsRef<str>, K: AsRef<str>, V: AsRef<str>>(
        &self,
        script: S,
        keys: &[K],
        args: &[V],
    ) -> Result<Vec<Option<String>>, redis::RedisError> {
        let value = self.eval_script(script, keys, args).await?;
        redis::from_redis_value(value).map_err(|error| {
            redis::RedisError::from((
                redis::ErrorKind::Parse,
                "unable to parse Redis script response",
                error.to_string(),
            ))
        })
    }
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
    use redis::{Arg, Cmd, Pipeline, RedisFuture, Value, aio::ConnectionLike};

    use super::{
        RedisOperation, prepare_mget_command, prepare_script_invocation, redis_result_label,
    };

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
                        _ => None,
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

    #[test]
    fn mget_command_preserves_input_key_order() {
        let command = prepare_mget_command(&["online-user:c", "online-user:a"]);
        let args = command
            .args_iter()
            .filter_map(|arg| match arg {
                Arg::Simple(value) => Some(value.to_vec()),
                Arg::Cursor => None,
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                b"MGET".to_vec(),
                b"online-user:c".to_vec(),
                b"online-user:a".to_vec(),
            ]
        );
    }

    #[test]
    fn tracing_uses_only_fixed_operation_and_result_labels() {
        assert_eq!(RedisOperation::Get.as_str(), "GET");
        assert_eq!(RedisOperation::Eval.as_str(), "EVAL");
        assert_eq!(
            RedisOperation::DeleteByPattern.as_str(),
            "DELETE_BY_PATTERN"
        );

        let result = Err::<(), _>(redis::RedisError::from((
            redis::ErrorKind::Io,
            "sensitive redis payload",
        )));
        assert_eq!(redis_result_label(&result), "error");
    }
}
