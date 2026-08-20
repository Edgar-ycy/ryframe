use redis::AsyncCommands;

use crate::RedisClient;

const KEY_PREFIX: &str = "ryframe:v0.7:idempotency:";

#[derive(Debug, Eq, PartialEq)]
pub enum RemoteIdempotencyReservation {
    Acquired,
    Processing,
    Conflict,
    Completed(String),
    NonReplayable,
}

/// Redis 幂等记录实现，只负责持久化状态机，不解释 HTTP 请求或响应。
#[derive(Clone)]
pub struct RedisIdempotencyStore {
    redis: RedisClient,
}

impl RedisIdempotencyStore {
    pub const fn new(redis: RedisClient) -> Self {
        Self { redis }
    }

    pub async fn reserve(
        &self,
        key: &str,
        fingerprint: &str,
        processing_ttl_secs: u64,
    ) -> Result<RemoteIdempotencyReservation, String> {
        let meta_key = self.redis.scoped_key(&meta_key(key));
        let guard_key = self.redis.scoped_key(&guard_key(key));
        let watched = [meta_key.clone(), guard_key.clone()];
        let fingerprint = fingerprint.to_owned();
        let result = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let meta_key = meta_key.clone();
                let guard_key = guard_key.clone();
                let fingerprint = fingerprint.clone();
                async move {
                    let exists: bool = connection.exists(&meta_key).await?;
                    if exists {
                        let existing: Option<String> =
                            connection.hget(&meta_key, "fingerprint").await?;
                        if existing.as_deref() != Some(fingerprint.as_str()) {
                            return Ok(Some(2_i64));
                        }
                        let state: Option<String> = connection.hget(&meta_key, "state").await?;
                        return Ok(Some(match state.as_deref() {
                            Some("processing") => 3,
                            Some("non_replayable") => 4,
                            Some("completed") => 5,
                            _ => 6,
                        }));
                    }
                    let guard: Option<String> = connection.get(&guard_key).await?;
                    if let Some(guard) = guard {
                        return Ok(Some(if guard == fingerprint { 4 } else { 2 }));
                    }
                    transaction
                        .hset(&meta_key, "state", "processing")
                        .ignore()
                        .hset(&meta_key, "fingerprint", fingerprint)
                        .ignore()
                        .expire(&meta_key, redis_ttl_secs(processing_ttl_secs))
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| 1_i64))
                }
            })
            .await
            .map_err(|error| format!("Redis 幂等保留失败: {error}"))?;

        match result {
            1 => Ok(RemoteIdempotencyReservation::Acquired),
            2 => Ok(RemoteIdempotencyReservation::Conflict),
            3 => Ok(RemoteIdempotencyReservation::Processing),
            4 => Ok(RemoteIdempotencyReservation::NonReplayable),
            5 => self
                .redis
                .get(response_key(key))
                .await
                .map_err(|error| format!("读取 Redis 幂等响应失败: {error}"))?
                .map(RemoteIdempotencyReservation::Completed)
                .ok_or_else(|| "已完成的幂等响应不存在".to_owned()),
            value => Err(format!("Redis 返回未知幂等状态: {value}")),
        }
    }

    pub async fn begin_execution(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        let meta_key = self.redis.scoped_key(&meta_key(key));
        let guard_key = self.redis.scoped_key(&guard_key(key));
        let watched = [meta_key.clone(), guard_key.clone()];
        let fingerprint = fingerprint.to_owned();
        match self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let meta_key = meta_key.clone();
                let guard_key = guard_key.clone();
                let fingerprint = fingerprint.clone();
                async move {
                    let stored_fingerprint: Option<String> =
                        connection.hget(&meta_key, "fingerprint").await?;
                    let state: Option<String> = connection.hget(&meta_key, "state").await?;
                    if stored_fingerprint.as_deref() != Some(fingerprint.as_str())
                        || state.as_deref() != Some("processing")
                    {
                        return Ok(Some(false));
                    }
                    transaction
                        .set_ex(&guard_key, fingerprint, completed_ttl_secs)
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("幂等执行保护被拒绝".into()),
            Err(error) => Err(format!("Redis 幂等执行保护失败: {error}")),
        }
    }

    pub async fn complete(
        &self,
        key: &str,
        fingerprint: &str,
        response: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        let meta_key = self.redis.scoped_key(&meta_key(key));
        let response_key = self.redis.scoped_key(&response_key(key));
        let guard_key = self.redis.scoped_key(&guard_key(key));
        let watched = [meta_key.clone(), response_key.clone(), guard_key.clone()];
        let fingerprint = fingerprint.to_owned();
        let response = response.to_owned();
        match self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let meta_key = meta_key.clone();
                let response_key = response_key.clone();
                let guard_key = guard_key.clone();
                let fingerprint = fingerprint.clone();
                let response = response.clone();
                async move {
                    let stored_fingerprint: Option<String> =
                        connection.hget(&meta_key, "fingerprint").await?;
                    if stored_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                        return Ok(Some(false));
                    }
                    transaction
                        .set_ex(&response_key, response, completed_ttl_secs)
                        .ignore()
                        .hset(&meta_key, "state", "completed")
                        .ignore()
                        .expire(&meta_key, redis_ttl_secs(completed_ttl_secs))
                        .ignore()
                        .del(&guard_key)
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("幂等完成状态被拒绝".into()),
            Err(error) => Err(format!("Redis 幂等完成写入失败: {error}")),
        }
    }

    pub async fn mark_non_replayable(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        let meta_key = self.redis.scoped_key(&meta_key(key));
        let watched = [meta_key.clone()];
        let fingerprint = fingerprint.to_owned();
        match self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let meta_key = meta_key.clone();
                let fingerprint = fingerprint.clone();
                async move {
                    let stored_fingerprint: Option<String> =
                        connection.hget(&meta_key, "fingerprint").await?;
                    if stored_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                        return Ok(Some(false));
                    }
                    transaction
                        .hset(&meta_key, "state", "non_replayable")
                        .ignore()
                        .expire(&meta_key, redis_ttl_secs(completed_ttl_secs))
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err("不可重放标记被拒绝".into()),
            Err(error) => Err(format!("Redis 幂等标记写入失败: {error}")),
        }
    }

    pub async fn release(&self, key: &str) {
        let _ = self.redis.del(meta_key(key)).await;
        let _ = self.redis.del(response_key(key)).await;
        let _ = self.redis.del(guard_key(key)).await;
    }
}

fn meta_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:meta")
}

fn response_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:response")
}

fn guard_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}:guard")
}

fn redis_ttl_secs(ttl_secs: u64) -> i64 {
    ttl_secs.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::{guard_key, meta_key, redis_ttl_secs, response_key};

    #[test]
    fn keys_keep_stable_namespace_and_distinct_suffixes() {
        assert_eq!(meta_key("request"), "ryframe:v0.7:idempotency:request:meta");
        assert_eq!(
            response_key("request"),
            "ryframe:v0.7:idempotency:request:response"
        );
        assert_eq!(
            guard_key("request"),
            "ryframe:v0.7:idempotency:request:guard"
        );
    }

    #[test]
    fn redis_ttl_is_bounded_to_signed_range() {
        assert_eq!(redis_ttl_secs(u64::MAX), i64::MAX);
    }
}
