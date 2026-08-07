use async_trait::async_trait;
use ryframe_core::RedisClient;

use super::keyspace::{
    namespace_values_hash_key, namespace_version_key, snapshot_hash_key, tenant_epoch_key,
    tenant_value_hash_key, user_version_key,
};
use super::*;

const READ_SNAPSHOT_SCRIPT: &str = r#"
local tenant_epoch = redis.call('GET', KEYS[1])
local user_version = redis.call('GET', KEYS[2])
if not tenant_epoch or not user_version then
  return {tenant_epoch or false, user_version or false, false}
end
local snapshot = redis.call('HGET', KEYS[3], tenant_epoch .. ':' .. user_version)
return {tenant_epoch, user_version, snapshot or false}
"#;

const WRITE_SNAPSHOT_SCRIPT: &str = r#"
local tenant_epoch = redis.call('GET', KEYS[1])
local user_version = redis.call('GET', KEYS[2])
local expected_epoch = tonumber(ARGV[1])
local expected_version = tonumber(ARGV[2])
if tenant_epoch and tonumber(tenant_epoch) > expected_epoch then
  return 0
end
if user_version and tonumber(user_version) > expected_version then
  return 0
end
redis.call('SET', KEYS[1], ARGV[1])
redis.call('SET', KEYS[2], ARGV[2])
redis.call('DEL', KEYS[3])
redis.call('HSET', KEYS[3], ARGV[1] .. ':' .. ARGV[2], ARGV[4])
redis.call('EXPIRE', KEYS[3], ARGV[3])
return 1
"#;

const UPDATE_MIRROR_SCRIPT: &str = r#"
local current = redis.call('GET', KEYS[1])
local incoming = tonumber(ARGV[1])
if current and tonumber(current) > incoming then
  return tonumber(current)
end
redis.call('SET', KEYS[1], ARGV[1])
return incoming
"#;

const READ_TENANT_VALUE_SCRIPT: &str = r#"
local tenant_epoch = redis.call('GET', KEYS[1])
if not tenant_epoch then
  return {false, false}
end
local value = redis.call('HGET', KEYS[2], tenant_epoch)
return {tenant_epoch, value or false}
"#;

const WRITE_TENANT_VALUE_SCRIPT: &str = r#"
local tenant_epoch = redis.call('GET', KEYS[1])
if not tenant_epoch or tenant_epoch ~= ARGV[1] then
  return 0
end
redis.call('DEL', KEYS[2])
redis.call('HSET', KEYS[2], ARGV[1], ARGV[3])
redis.call('EXPIRE', KEYS[2], ARGV[2])
return 1
"#;

const READ_NAMESPACE_VALUE_SCRIPT: &str = r#"
local namespace_version = redis.call('GET', KEYS[1])
if not namespace_version then
  return {false, false}
end
local value = redis.call('HGET', KEYS[2], ARGV[1])
return {namespace_version, value or false}
"#;

const WRITE_NAMESPACE_VALUE_SCRIPT: &str = r#"
local namespace_version = redis.call('GET', KEYS[1])
if not namespace_version or namespace_version ~= ARGV[1] then
  return 0
end
redis.call('HSET', KEYS[2], ARGV[2], ARGV[4])
redis.call('EXPIRE', KEYS[2], ARGV[3])
return 1
"#;

const ADVANCE_NAMESPACE_VERSION_SCRIPT: &str = r#"
local function is_canonical_decimal(value)
  if not value or value == '' then
    return false
  end
  if value == '0' then
    return true
  end
  if string.sub(value, 1, 1) == '0' then
    return false
  end
  return string.find(value, '[^0-9]') == nil
end

local function compare_decimal(left, right)
  if string.len(left) < string.len(right) then
    return -1
  end
  if string.len(left) > string.len(right) then
    return 1
  end
  if left < right then
    return -1
  end
  if left > right then
    return 1
  end
  return 0
end

local incoming = ARGV[1]
if not is_canonical_decimal(incoming) then
  return redis.error_reply('namespace version must be a canonical decimal string')
end

local current = redis.call('GET', KEYS[1])
if current then
  if not is_canonical_decimal(current) then
    return redis.error_reply('stored namespace version is not a canonical decimal string')
  end
  if compare_decimal(incoming, current) <= 0 then
    return 0
  end
end

redis.call('SET', KEYS[1], incoming)
redis.call('DEL', KEYS[2])
return 1
"#;

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
        // 三个键共享同一租户 hash tag，Redis Cluster 可把整段脚本固定路由到一个槽。
        let keys = [
            tenant_epoch_key(tenant_id),
            user_version_key(tenant_id, user_id),
            snapshot_hash_key(tenant_id, user_id),
        ];
        let args: [String; 0] = [];
        let values = self
            .redis
            .eval_script_optional_strings(READ_SNAPSHOT_SCRIPT, &keys, &args)
            .await
            .map_err(|error| error.to_string())?;
        if values.len() != 3 {
            return Err(format!("授权快照脚本返回了 {} 项，预期 3 项", values.len()));
        }
        let tenant_authorization_epoch = parse_optional_version(values[0].as_deref())?;
        let user_authorization_version = parse_optional_version(values[1].as_deref())?;
        let snapshot = values[2]
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
        let keys = [
            tenant_epoch_key(tenant_id),
            user_version_key(tenant_id, user_id),
            snapshot_hash_key(tenant_id, user_id),
        ];
        let payload = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
        let args = [
            versions.tenant_authorization_epoch.to_string(),
            versions.user_authorization_version.to_string(),
            AUTHORIZATION_SNAPSHOT_TTL_SECS.to_string(),
            payload,
        ];
        self.redis
            .eval_script_i64(WRITE_SNAPSHOT_SCRIPT, &keys, &args)
            .await
            .map(|stored| stored == 1)
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
        let keys = [
            tenant_epoch_key(tenant_id),
            tenant_value_hash_key(tenant_id, namespace),
        ];
        let args: [String; 0] = [];
        let values = self
            .redis
            .eval_script_optional_strings(READ_TENANT_VALUE_SCRIPT, &keys, &args)
            .await
            .map_err(|error| error.to_string())?;
        if values.len() != 2 {
            return Err(format!("租户缓存脚本返回了 {} 项，预期 2 项", values.len()));
        }
        let Some(tenant_authorization_epoch) = parse_optional_version(values[0].as_deref())? else {
            return Ok(None);
        };
        Ok(Some(TenantCacheLookup {
            tenant_authorization_epoch,
            value: values[1].clone(),
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
        let keys = [
            tenant_epoch_key(tenant_id),
            tenant_value_hash_key(tenant_id, namespace),
        ];
        let args = [
            authorization_epoch.to_string(),
            ttl_secs.to_string(),
            value.to_owned(),
        ];
        self.redis
            .eval_script_i64(WRITE_TENANT_VALUE_SCRIPT, &keys, &args)
            .await
            .map(|stored| stored == 1)
            .map_err(|error| error.to_string())
    }

    async fn update_namespace_version(
        &self,
        tenant_id: &str,
        namespace: &str,
        namespace_version: i64,
    ) -> Result<(), String> {
        let keys = [
            namespace_version_key(tenant_id, namespace),
            namespace_values_hash_key(tenant_id, namespace),
        ];
        let args = [namespace_version.to_string()];
        self.redis
            .eval_script_i64(ADVANCE_NAMESPACE_VERSION_SCRIPT, &keys, &args)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn read_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
    ) -> Result<Option<NamespaceCacheLookup>, String> {
        let keys = [
            namespace_version_key(tenant_id, namespace),
            namespace_values_hash_key(tenant_id, namespace),
        ];
        let args = [item.to_owned()];
        let values = self
            .redis
            .eval_script_optional_strings(READ_NAMESPACE_VALUE_SCRIPT, &keys, &args)
            .await
            .map_err(|error| error.to_string())?;
        if values.len() != 2 {
            return Err(format!(
                "租户缓存命名空间脚本返回了 {} 项，预期 2 项",
                values.len()
            ));
        }
        let Some(raw_version) = values[0].as_deref() else {
            return Ok(None);
        };
        let namespace_version = raw_version
            .parse::<i64>()
            .map_err(|error| format!("租户缓存命名空间版本无效: {error}"))?;
        if namespace_version < 0 || raw_version != namespace_version.to_string() {
            return Err("租户缓存命名空间版本不是规范十进制字符串".into());
        }
        Ok(Some(NamespaceCacheLookup {
            namespace_version,
            value: values[1].clone(),
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
        let keys = [
            namespace_version_key(tenant_id, namespace),
            namespace_values_hash_key(tenant_id, namespace),
        ];
        let args = [
            namespace_version.to_string(),
            item.to_owned(),
            ttl_secs.to_string(),
            value.to_owned(),
        ];
        self.redis
            .eval_script_i64(WRITE_NAMESPACE_VALUE_SCRIPT, &keys, &args)
            .await
            .map(|stored| stored == 1)
            .map_err(|error| error.to_string())
    }
}

async fn update_mirror(redis: &RedisClient, key: String, version: i64) -> Result<(), String> {
    let keys = [key];
    let args = [version.to_string()];
    redis
        .eval_script_i64(UPDATE_MIRROR_SCRIPT, &keys, &args)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
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
