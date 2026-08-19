use async_trait::async_trait;
use redis::AsyncCommands;
use ryframe_adapters::RedisClient;

use super::keyspace::{
    namespace_values_hash_key, namespace_version_key, snapshot_hash_key, tenant_epoch_key,
    tenant_value_hash_key, user_version_key,
};
use super::*;

pub(super) struct RedisAuthorizationCacheBackend {
    pub(super) redis: RedisClient,
}

#[async_trait]
impl AuthorizationCacheBackend for RedisAuthorizationCacheBackend {
    async fn lookup_snapshot(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> Result<AuthorizationCacheLookup, String> {
        let tenant_key = self.redis.scoped_key(&tenant_epoch_key(tenant_id));
        let user_key = self.redis.scoped_key(&user_version_key(tenant_id, user_id));
        let snapshot_key = self
            .redis
            .scoped_key(&snapshot_hash_key(tenant_id, user_id));
        let watched = [tenant_key.clone(), user_key.clone(), snapshot_key.clone()];
        let values: (Option<String>, Option<String>, Option<String>) = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let tenant_key = tenant_key.clone();
                let user_key = user_key.clone();
                let snapshot_key = snapshot_key.clone();
                async move {
                    let tenant_epoch: Option<String> = connection.get(&tenant_key).await?;
                    let user_version: Option<String> = connection.get(&user_key).await?;
                    let field = match (&tenant_epoch, &user_version) {
                        (Some(tenant_epoch), Some(user_version)) => {
                            format!("{tenant_epoch}:{user_version}")
                        }
                        _ => "__ryframe_missing_snapshot__".to_owned(),
                    };
                    transaction
                        .get(&tenant_key)
                        .get(&user_key)
                        .hget(&snapshot_key, field);
                    transaction.query_async(&mut connection).await
                }
            })
            .await
            .map_err(|error| error.to_string())?;
        let tenant_authorization_epoch = parse_optional_version(values.0.as_deref())?;
        let user_authorization_version = parse_optional_version(values.1.as_deref())?;
        let snapshot = values
            .2
            .as_deref()
            .map(serde_json::from_str::<AuthorizationSnapshot>)
            .transpose()
            .map_err(|error| format!("授权快照 JSON 无效: {error}"))?;
        if let Some(snapshot) = &snapshot
            && (Some(snapshot.versions.tenant_authorization_epoch) != tenant_authorization_epoch
                || Some(snapshot.versions.user_authorization_version) != user_authorization_version)
        {
            return Err("授权快照内版本与 Redis 镜像不一致".into());
        }
        Ok(AuthorizationCacheLookup {
            tenant_authorization_epoch,
            user_authorization_version,
            snapshot,
        })
    }

    async fn store_snapshot(&self, snapshot: &AuthorizationSnapshot) -> Result<bool, String> {
        let tenant_id = &snapshot.principal.actor.tenant_id;
        let user_id = snapshot.principal.actor.user_id;
        let versions = snapshot.versions;
        let tenant_key = self.redis.scoped_key(&tenant_epoch_key(tenant_id));
        let user_key = self.redis.scoped_key(&user_version_key(tenant_id, user_id));
        let snapshot_key = self
            .redis
            .scoped_key(&snapshot_hash_key(tenant_id, user_id));
        let watched = [tenant_key.clone(), user_key.clone(), snapshot_key.clone()];
        let payload = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
        let tenant_epoch = versions.tenant_authorization_epoch.to_string();
        let user_version = versions.user_authorization_version.to_string();
        let ttl_secs = AUTHORIZATION_SNAPSHOT_TTL_SECS;
        self.redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let tenant_key = tenant_key.clone();
                let user_key = user_key.clone();
                let snapshot_key = snapshot_key.clone();
                let payload = payload.clone();
                let tenant_epoch = tenant_epoch.clone();
                let user_version = user_version.clone();
                async move {
                    let current_tenant: Option<String> = connection.get(&tenant_key).await?;
                    let current_user: Option<String> = connection.get(&user_key).await?;
                    if version_is_newer(current_tenant.as_deref(), &tenant_epoch)
                        .map_err(redis_cache_value_error)?
                        || version_is_newer(current_user.as_deref(), &user_version)
                            .map_err(redis_cache_value_error)?
                    {
                        return Ok(Some(false));
                    }
                    transaction
                        .set(&tenant_key, &tenant_epoch)
                        .ignore()
                        .set(&user_key, &user_version)
                        .ignore()
                        .del(&snapshot_key)
                        .ignore()
                        .hset(
                            &snapshot_key,
                            format!("{tenant_epoch}:{user_version}"),
                            payload,
                        )
                        .ignore()
                        .expire(&snapshot_key, redis_ttl_secs(ttl_secs))
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn update_tenant_epoch(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
    ) -> Result<(), String> {
        update_mirror(
            &self.redis,
            tenant_epoch_key(tenant_id),
            i64::from(authorization_epoch),
        )
        .await
    }

    async fn update_user_version(
        &self,
        tenant_id: &str,
        user_id: i64,
        authorization_version: i32,
    ) -> Result<(), String> {
        update_mirror(
            &self.redis,
            user_version_key(tenant_id, user_id),
            i64::from(authorization_version),
        )
        .await
    }

    async fn read_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
    ) -> Result<Option<TenantCacheLookup>, String> {
        let tenant_key = self.redis.scoped_key(&tenant_epoch_key(tenant_id));
        let value_key = self
            .redis
            .scoped_key(&tenant_value_hash_key(tenant_id, namespace));
        let watched = [tenant_key.clone(), value_key.clone()];
        let values: (Option<String>, Option<String>) = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let tenant_key = tenant_key.clone();
                let value_key = value_key.clone();
                async move {
                    let tenant_epoch: Option<String> = connection.get(&tenant_key).await?;
                    let field = tenant_epoch
                        .as_deref()
                        .unwrap_or("__ryframe_missing_tenant_epoch__");
                    transaction.get(&tenant_key).hget(&value_key, field);
                    transaction.query_async(&mut connection).await
                }
            })
            .await
            .map_err(|error| error.to_string())?;
        let Some(tenant_authorization_epoch) = parse_optional_version(values.0.as_deref())? else {
            return Ok(None);
        };
        Ok(Some(TenantCacheLookup {
            tenant_authorization_epoch,
            value: values.1,
        }))
    }

    async fn store_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        authorization_epoch: i32,
        value: &str,
        ttl_secs: u64,
    ) -> Result<bool, String> {
        let tenant_key = self.redis.scoped_key(&tenant_epoch_key(tenant_id));
        let value_key = self
            .redis
            .scoped_key(&tenant_value_hash_key(tenant_id, namespace));
        let watched = [tenant_key.clone(), value_key.clone()];
        let epoch = authorization_epoch.to_string();
        let value = value.to_owned();
        self.redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let tenant_key = tenant_key.clone();
                let value_key = value_key.clone();
                let epoch = epoch.clone();
                let value = value.clone();
                async move {
                    let current: Option<String> = connection.get(&tenant_key).await?;
                    if current.as_deref() != Some(epoch.as_str()) {
                        return Ok(Some(false));
                    }
                    transaction
                        .del(&value_key)
                        .ignore()
                        .hset(&value_key, &epoch, value)
                        .ignore()
                        .expire(&value_key, redis_ttl_secs(ttl_secs))
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn update_namespace_version(
        &self,
        tenant_id: &str,
        namespace: &str,
        namespace_version: i64,
    ) -> Result<(), String> {
        let version_key = self
            .redis
            .scoped_key(&namespace_version_key(tenant_id, namespace));
        let values_key = self
            .redis
            .scoped_key(&namespace_values_hash_key(tenant_id, namespace));
        let watched = [version_key.clone(), values_key.clone()];
        let incoming = namespace_version.to_string();
        validate_canonical_decimal(&incoming)?;
        self.redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let version_key = version_key.clone();
                let values_key = values_key.clone();
                let incoming = incoming.clone();
                async move {
                    let current: Option<String> = connection.get(&version_key).await?;
                    if let Some(current) = current.as_deref() {
                        validate_canonical_decimal(current).map_err(redis_cache_value_error)?;
                        if compare_decimal(&incoming, current).is_le() {
                            return Ok(Some(()));
                        }
                    }
                    transaction
                        .set(&version_key, incoming)
                        .ignore()
                        .del(&values_key)
                        .ignore();
                    transaction.query_async(&mut connection).await
                }
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn read_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
    ) -> Result<Option<NamespaceCacheLookup>, String> {
        let version_key = self
            .redis
            .scoped_key(&namespace_version_key(tenant_id, namespace));
        let values_key = self
            .redis
            .scoped_key(&namespace_values_hash_key(tenant_id, namespace));
        let watched = [version_key.clone(), values_key.clone()];
        let item = item.to_owned();
        let values: (Option<String>, Option<String>) = self
            .redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let version_key = version_key.clone();
                let values_key = values_key.clone();
                let item = item.clone();
                async move {
                    transaction.get(&version_key).hget(&values_key, item);
                    transaction.query_async(&mut connection).await
                }
            })
            .await
            .map_err(|error| error.to_string())?;
        let Some(raw_version) = values.0.as_deref() else {
            return Ok(None);
        };
        validate_canonical_decimal(raw_version)?;
        let namespace_version = raw_version
            .parse::<i64>()
            .map_err(|error| format!("租户缓存命名空间版本无效: {error}"))?;
        Ok(Some(NamespaceCacheLookup {
            namespace_version,
            value: values.1,
        }))
    }

    async fn store_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
        namespace_version: i64,
        value: &str,
        ttl_secs: u64,
    ) -> Result<bool, String> {
        let version_key = self
            .redis
            .scoped_key(&namespace_version_key(tenant_id, namespace));
        let values_key = self
            .redis
            .scoped_key(&namespace_values_hash_key(tenant_id, namespace));
        let watched = [version_key.clone(), values_key.clone()];
        let expected_version = namespace_version.to_string();
        let item = item.to_owned();
        let value = value.to_owned();
        self.redis
            .transaction(&watched, move |mut connection, mut transaction| {
                let version_key = version_key.clone();
                let values_key = values_key.clone();
                let expected_version = expected_version.clone();
                let item = item.clone();
                let value = value.clone();
                async move {
                    let current: Option<String> = connection.get(&version_key).await?;
                    if current.as_deref() != Some(expected_version.as_str()) {
                        return Ok(Some(false));
                    }
                    transaction
                        .hset(&values_key, item, value)
                        .ignore()
                        .expire(&values_key, redis_ttl_secs(ttl_secs))
                        .ignore();
                    let committed: Option<()> = transaction.query_async(&mut connection).await?;
                    Ok(committed.map(|()| true))
                }
            })
            .await
            .map_err(|error| error.to_string())
    }
}

async fn update_mirror(redis: &RedisClient, key: String, version: i64) -> Result<(), String> {
    let key = redis.scoped_key(&key);
    let watched = [key.clone()];
    let incoming = version.to_string();
    redis
        .transaction(&watched, move |mut connection, mut transaction| {
            let key = key.clone();
            let incoming = incoming.clone();
            async move {
                let current: Option<String> = connection.get(&key).await?;
                if version_is_newer(current.as_deref(), &incoming)
                    .map_err(redis_cache_value_error)?
                {
                    return Ok(Some(()));
                }
                transaction.set(&key, incoming).ignore();
                transaction.query_async(&mut connection).await
            }
        })
        .await
        .map_err(|error| error.to_string())
}

fn version_is_newer(current: Option<&str>, incoming: &str) -> Result<bool, String> {
    current
        .map(|current| {
            let current = current
                .parse::<i64>()
                .map_err(|error| format!("授权版本不是有效整数: {error}"))?;
            let incoming = incoming
                .parse::<i64>()
                .map_err(|error| format!("授权版本不是有效整数: {error}"))?;
            Ok(current > incoming)
        })
        .transpose()
        .map(|value| value.unwrap_or(false))
}

fn validate_canonical_decimal(value: &str) -> Result<(), String> {
    let canonical = value == "0"
        || (!value.is_empty()
            && !value.starts_with('0')
            && value.bytes().all(|byte| byte.is_ascii_digit()));
    if canonical {
        Ok(())
    } else {
        Err("租户缓存命名空间版本不是规范十进制字符串".into())
    }
}

fn compare_decimal(left: &str, right: &str) -> std::cmp::Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

fn parse_optional_version(value: Option<&str>) -> Result<Option<i32>, String> {
    value
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|error| format!("授权版本不是有效整数: {error}"))
        })
        .transpose()
}

fn redis_ttl_secs(ttl_secs: u64) -> i64 {
    ttl_secs.min(i64::MAX as u64) as i64
}

fn redis_cache_value_error(message: String) -> redis::RedisError {
    (
        redis::ErrorKind::UnexpectedReturnType,
        "授权缓存 Redis 值无效",
        message,
    )
        .into()
}
