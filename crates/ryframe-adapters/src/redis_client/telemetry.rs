use std::{future::Future, time::Instant};

use tracing::Instrument;

/// Redis 客户端 span 使用的固定操作集合，禁止将键和参数内容作为属性。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RedisOperation {
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
    pub(super) const fn as_str(self) -> &'static str {
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

pub(super) async fn trace_redis_operation<T>(
    operation: RedisOperation,
    future: impl Future<Output = Result<T, redis::RedisError>>,
) -> Result<T, redis::RedisError> {
    let started = Instant::now();
    let span = redis_operation_span(operation);
    let result = future.instrument(span.clone()).await;
    let result_label = redis_result_label(&result);
    span.record("redis.result", result_label);
    crate::metrics::observe_connector_operation(
        "redis",
        operation.as_str(),
        result_label,
        started.elapsed(),
    );
    result
}

fn redis_result_label<T>(result: &Result<T, redis::RedisError>) -> &'static str {
    if result.is_ok() { "success" } else { "error" }
}
