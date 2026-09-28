use redis::{
    AsyncConnectionConfig, Cmd, FromRedisValue, Pipeline, RedisFuture, Value,
    aio::{ConnectionLike, MultiplexedConnection},
};
use std::sync::Mutex;
use tokio::sync::Semaphore;

use super::{RedisClient, connection::redis_timeout_error, telemetry};

pub(super) struct RedisTransactionPool {
    idle: Mutex<Vec<MultiplexedConnection>>,
    permits: Semaphore,
}

impl RedisTransactionPool {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            permits: Semaphore::new(capacity),
        }
    }

    fn checkout(&self) -> Option<MultiplexedConnection> {
        match self.idle.lock() {
            Ok(mut idle) => idle.pop(),
            Err(poisoned) => poisoned.into_inner().pop(),
        }
    }

    fn checkin(&self, connection: MultiplexedConnection) {
        match self.idle.lock() {
            Ok(mut idle) => idle.push(connection),
            Err(poisoned) => poisoned.into_inner().push(connection),
        }
    }
}

/// 事务闭包使用的独占 Redis 连接借用。
///
/// 该类型不提供克隆、底层句柄访问或所有权转换，闭包返回后不能继续持有连接。
pub struct RedisTransactionConnection<'a> {
    inner: &'a mut MultiplexedConnection,
}

impl<'a> RedisTransactionConnection<'a> {
    const fn new(inner: &'a mut MultiplexedConnection) -> Self {
        Self { inner }
    }
}

impl ConnectionLike for RedisTransactionConnection<'_> {
    fn req_packed_command<'a>(&'a mut self, command: &'a Cmd) -> RedisFuture<'a, Value> {
        self.inner.req_packed_command(command)
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        self.inner.req_packed_commands(pipeline, offset, count)
    }

    fn get_db(&self) -> i64 {
        self.inner.get_db()
    }
}

impl RedisClient {
    /// 从有界连接池租用独占连接执行乐观事务，仅 WATCH 冲突允许重试。
    ///
    /// 闭包可能执行多次，只能进行可重复的 Redis 读取和事务命令构造，不能产生外部副作用。
    /// 连接在事务中途不重连；错误、超时和取消均丢弃该代次，防止下一次继承 WATCH 状态。
    pub async fn transaction<K, T, F>(
        &self,
        keys: &[K],
        mut operation: F,
    ) -> Result<T, redis::RedisError>
    where
        K: AsRef<str>,
        T: FromRedisValue,
        F: for<'a> AsyncFnMut(
            RedisTransactionConnection<'a>,
            Pipeline,
        ) -> Result<Option<T>, redis::RedisError>,
    {
        if keys.is_empty() {
            return Err(redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "Redis 事务至少需要一个 WATCH 键",
            )));
        }
        let keys = keys
            .iter()
            .map(|key| self.scoped_key(key.as_ref()))
            .collect::<Vec<_>>();
        telemetry::trace_redis_operation(telemetry::RedisOperation::Transaction, async {
            let deadline = tokio::time::Instant::now() + self.timeout;
            let _permit =
                tokio::time::timeout_at(deadline, self.transaction_pool.permits.acquire())
                    .await
                    .map_err(|_| redis_timeout_error("Redis 事务等待、连接或执行超时"))?
                    .map_err(|_| {
                        redis::RedisError::from((
                            redis::ErrorKind::Client,
                            "Redis 事务连接池已关闭",
                        ))
                    })?;
            let checked_out = self.transaction_pool.checkout();
            let mut reused_connection = checked_out.is_some();
            let mut connection = match checked_out {
                Some(connection) => connection,
                None => tokio::time::timeout_at(deadline, self.open_transaction_connection())
                    .await
                    .map_err(|_| redis_timeout_error("Redis 事务等待、连接或执行超时"))??,
            };

            loop {
                if let Err(error) = watch_until(deadline, &mut connection, &keys).await {
                    if reused_connection && error.is_connection_dropped() {
                        connection =
                            tokio::time::timeout_at(deadline, self.open_transaction_connection())
                                .await
                                .map_err(|_| {
                                    redis_timeout_error("Redis 事务等待、连接或执行超时")
                                })??;
                        reused_connection = false;
                        continue;
                    }
                    return Err(error);
                }
                reused_connection = false;
                let mut pipeline = redis::pipe();
                pipeline.atomic();
                let transaction_connection = RedisTransactionConnection::new(&mut connection);
                let response =
                    tokio::time::timeout_at(deadline, operation(transaction_connection, pipeline))
                        .await
                        .map_err(|_| redis_timeout_error("Redis 事务等待、连接或执行超时"))??;

                let clean = clear_watch_until(deadline, &mut connection).await;
                if let Some(result) = response {
                    if clean {
                        self.transaction_pool.checkin(connection);
                    }
                    return Ok(result);
                }
                if !clean {
                    connection =
                        tokio::time::timeout_at(deadline, self.open_transaction_connection())
                            .await
                            .map_err(|_| redis_timeout_error("Redis 事务等待、连接或执行超时"))??;
                }
            }
        })
        .await
    }

    async fn open_transaction_connection(
        &self,
    ) -> Result<MultiplexedConnection, redis::RedisError> {
        let config = AsyncConnectionConfig::new()
            .set_connection_timeout(Some(self.timeout))
            .set_response_timeout(Some(self.timeout));
        self.client
            .get_multiplexed_async_connection_with_config(&config)
            .await
    }
}

async fn watch_until(
    deadline: tokio::time::Instant,
    connection: &mut MultiplexedConnection,
    keys: &[String],
) -> Result<(), redis::RedisError> {
    tokio::time::timeout_at(
        deadline,
        redis::cmd("WATCH").arg(keys).exec_async(connection),
    )
    .await
    .map_err(|_| redis_timeout_error("Redis 事务等待、连接或执行超时"))?
}

async fn clear_watch_until(
    deadline: tokio::time::Instant,
    connection: &mut MultiplexedConnection,
) -> bool {
    matches!(
        tokio::time::timeout_at(deadline, redis::cmd("UNWATCH").exec_async(connection)).await,
        Ok(Ok(()))
    )
}
