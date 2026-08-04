use std::{fmt, sync::Arc};

use async_trait::async_trait;
use ryframe_auth::RequestPrincipal;
use ryframe_config::RedisMode;
use ryframe_core::RedisClient;
use ryframe_db::{
    CacheNamespaceVersionRepository, OutboxEventRepository, RecordOutboxEvent, TenantRepository,
    UserRepository, validate_cache_namespace,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseTransaction;
use serde::{Deserialize, Serialize};

pub const AUTHORIZATION_SNAPSHOT_TTL_SECS: u64 = 300;
pub const AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE: &str = "security.authorization.mirror-updated";

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

/// 与一个授权快照绑定的持久化版本。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationVersions {
    pub tenant_authorization_epoch: i32,
    pub user_authorization_version: i32,
}

/// Redis 中保存的完整请求授权快照。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizationSnapshot {
    pub versions: AuthorizationVersions,
    pub tenant_session_version: i32,
    pub principal: RequestPrincipal,
}

/// 一次 Lua 原子读取的结果。
#[derive(Clone, Debug)]
pub struct AuthorizationCacheLookup {
    pub tenant_authorization_epoch: Option<i32>,
    pub user_authorization_version: Option<i32>,
    pub snapshot: Option<AuthorizationSnapshot>,
}

impl AuthorizationCacheLookup {
    fn miss() -> Self {
        Self {
            tenant_authorization_epoch: None,
            user_authorization_version: None,
            snapshot: None,
        }
    }
}

/// 租户级版本化缓存的一次读取结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantCacheLookup {
    pub tenant_authorization_epoch: i32,
    pub value: Option<String>,
}

/// 独立租户缓存命名空间的一次版本化读取结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamespaceCacheLookup {
    pub namespace_version: i64,
    pub value: Option<String>,
}

/// Outbox 中持久化的授权版本镜像修复负载。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum AuthorizationMirrorUpdate {
    Tenant {
        tenant_id: String,
        authorization_epoch: i32,
    },
    User {
        tenant_id: String,
        user_id: i64,
        authorization_version: i32,
    },
    TenantCacheNamespace {
        tenant_id: String,
        namespace: String,
        namespace_version: i64,
    },
}

/// 权限缓存后端接口；生产实现只使用 Redis，测试可注入确定性 Mock。
#[async_trait]
pub trait AuthorizationCacheBackend: Send + Sync {
    async fn lookup_snapshot(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> Result<AuthorizationCacheLookup, String>;

    async fn store_snapshot(&self, snapshot: &AuthorizationSnapshot) -> Result<bool, String>;

    async fn update_tenant_epoch(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
    ) -> Result<(), String>;

    async fn update_user_version(
        &self,
        tenant_id: &str,
        user_id: i64,
        authorization_version: i32,
    ) -> Result<(), String>;

    async fn read_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
    ) -> Result<Option<TenantCacheLookup>, String>;

    async fn store_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        authorization_epoch: i32,
        value: &str,
        ttl_secs: u64,
    ) -> Result<bool, String>;

    async fn update_namespace_version(
        &self,
        tenant_id: &str,
        namespace: &str,
        namespace_version: i64,
    ) -> Result<(), String>;

    async fn read_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
    ) -> Result<Option<NamespaceCacheLookup>, String>;

    async fn store_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
        namespace_version: i64,
        value: &str,
        ttl_secs: u64,
    ) -> Result<bool, String>;
}

#[derive(Clone)]
pub struct AuthorizationCache {
    backend: Option<Arc<dyn AuthorizationCacheBackend>>,
    required: bool,
}

impl fmt::Debug for AuthorizationCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationCache")
            .field("enabled", &self.backend.is_some())
            .field("required", &self.required)
            .finish()
    }
}

impl AuthorizationCache {
    pub fn new(redis: Option<RedisClient>, mode: RedisMode) -> Self {
        let backend = redis.map(|redis| {
            Arc::new(RedisAuthorizationCacheBackend { redis }) as Arc<dyn AuthorizationCacheBackend>
        });
        Self {
            backend,
            required: mode.is_required(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            backend: None,
            required: false,
        }
    }

    pub fn from_backend(backend: Arc<dyn AuthorizationCacheBackend>, required: bool) -> Self {
        Self {
            backend: Some(backend),
            required,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.backend.is_some()
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    /// 在业务事务内递增一组用户授权版本，并原子记录 Redis 镜像修复事件。
    pub async fn increment_user_versions_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<Vec<(i64, i32)>> {
        let mut user_ids = user_ids.to_vec();
        user_ids.sort_unstable();
        user_ids.dedup();
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let affected = UserRepository
            .increment_authorization_versions(transaction, tenant_id, &user_ids)
            .await?;
        if affected != user_ids.len() as u64 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        let versions = UserRepository
            .find_authorization_versions(transaction, tenant_id, &user_ids)
            .await?;
        if versions.len() != user_ids.len() {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        self.record_user_mirror_updates_in_transaction(transaction, tenant_id, &versions)
            .await?;
        Ok(versions)
    }

    /// 为已由调用方递增的单个用户版本记录镜像修复事件。
    pub async fn record_user_mirror_update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        user_id: i64,
        authorization_version: i32,
    ) -> AppResult<()> {
        self.record_user_mirror_updates_in_transaction(
            transaction,
            tenant_id,
            &[(user_id, authorization_version)],
        )
        .await
    }

    /// 在业务事务内递增租户授权纪元，并原子记录 Redis 镜像修复事件。
    pub async fn increment_tenant_epoch_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        let authorization_epoch = TenantRepository
            .increment_authorization_epoch_in_txn(transaction, tenant_id)
            .await?;
        if self.is_enabled() {
            let now = OutboxEventRepository.database_utc_now(transaction).await?;
            let payload = AuthorizationMirrorUpdate::Tenant {
                tenant_id: tenant_id.to_owned(),
                authorization_epoch,
            };
            OutboxEventRepository
                .record_in_transaction(
                    transaction,
                    mirror_event(
                        tenant_id,
                        "tenant",
                        tenant_id,
                        i64::from(authorization_epoch),
                        payload,
                        now,
                    )?,
                    now,
                )
                .await?;
        }
        Ok(authorization_epoch)
    }

    /// 在业务事务中递增数据库权威命名空间版本，并原子写入 Outbox。
    ///
    /// 即使 Redis 未启用，数据库计数器仍会推进；以后重新启用 Redis 时可以从数据库
    /// 恢复权威版本，而不需要猜测或扫描旧缓存键。
    pub async fn record_namespace_version_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        namespace: &str,
    ) -> AppResult<i64> {
        validate_cache_namespace(namespace)?;
        let namespace_version = CacheNamespaceVersionRepository
            .increment_in_transaction(transaction, tenant_id, namespace)
            .await?;
        let now = OutboxEventRepository.database_utc_now(transaction).await?;
        let payload = AuthorizationMirrorUpdate::TenantCacheNamespace {
            tenant_id: tenant_id.to_owned(),
            namespace: namespace.to_owned(),
            namespace_version,
        };
        OutboxEventRepository
            .record_in_transaction(
                transaction,
                mirror_event(
                    tenant_id,
                    "tenant_cache_namespace",
                    namespace,
                    namespace_version,
                    payload,
                    now,
                )?,
                now,
            )
            .await?;
        Ok(namespace_version)
    }

    async fn record_user_mirror_updates_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        versions: &[(i64, i32)],
    ) -> AppResult<()> {
        if !self.is_enabled() || versions.is_empty() {
            return Ok(());
        }
        let now = OutboxEventRepository.database_utc_now(transaction).await?;
        for (user_id, authorization_version) in versions {
            let aggregate_id = user_id.to_string();
            let payload = AuthorizationMirrorUpdate::User {
                tenant_id: tenant_id.to_owned(),
                user_id: *user_id,
                authorization_version: *authorization_version,
            };
            OutboxEventRepository
                .record_in_transaction(
                    transaction,
                    mirror_event(
                        tenant_id,
                        "user",
                        &aggregate_id,
                        i64::from(*authorization_version),
                        payload,
                        now,
                    )?,
                    now,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn lookup_snapshot(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<AuthorizationCacheLookup> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(AuthorizationCacheLookup::miss())
            };
        };
        match backend.lookup_snapshot(tenant_id, user_id).await {
            Ok(lookup) => Ok(lookup),
            Err(error) if self.required => {
                tracing::error!(tenant_id, user_id, %error, "授权快照原子读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, user_id, %error, "授权快照读取失败，回源主库");
                Ok(AuthorizationCacheLookup::miss())
            }
        }
    }

    pub async fn store_snapshot(&self, snapshot: &AuthorizationSnapshot) -> AppResult<bool> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(false)
            };
        };
        match backend.store_snapshot(snapshot).await {
            Ok(stored) => Ok(stored),
            Err(error) if self.required => {
                tracing::error!(
                    tenant_id = %snapshot.principal.actor.tenant_id,
                    user_id = snapshot.principal.actor.user_id,
                    %error,
                    "授权快照写入失败"
                );
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(
                    tenant_id = %snapshot.principal.actor.tenant_id,
                    user_id = snapshot.principal.actor.user_id,
                    %error,
                    "授权快照写入失败，本次使用主库结果"
                );
                // 主库结果已经完成完整校验；可选缓存故障不能把安全的强一致性结果降级为失败。
                Ok(true)
            }
        }
    }

    pub async fn sync_tenant_epoch(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
    ) -> AppResult<()> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(())
            };
        };
        self.handle_mirror_result(
            backend
                .update_tenant_epoch(tenant_id, authorization_epoch)
                .await,
            tenant_id,
            None,
        )
    }

    pub async fn sync_namespace_version(
        &self,
        tenant_id: &str,
        namespace: &str,
        namespace_version: i64,
    ) -> AppResult<()> {
        validate_cache_namespace(namespace)?;
        validate_namespace_version(namespace_version)?;
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(())
            };
        };
        self.handle_mirror_result(
            backend
                .update_namespace_version(tenant_id, namespace, namespace_version)
                .await,
            tenant_id,
            None,
        )
    }

    pub async fn sync_user_versions(
        &self,
        tenant_id: &str,
        versions: &[(i64, i32)],
    ) -> AppResult<()> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(())
            };
        };
        for (user_id, authorization_version) in versions {
            self.handle_mirror_result(
                backend
                    .update_user_version(tenant_id, *user_id, *authorization_version)
                    .await,
                tenant_id,
                Some(*user_id),
            )?;
        }
        Ok(())
    }

    /// Outbox Worker 使用严格模式修复镜像；失败必须保留事件并重试。
    pub async fn repair_tenant_epoch(
        &self,
        tenant_id: &str,
        authorization_epoch: i32,
    ) -> AppResult<()> {
        let backend = self.backend.as_ref().ok_or_else(cache_unavailable)?;
        backend
            .update_tenant_epoch(tenant_id, authorization_epoch)
            .await
            .map_err(|error| AppError::ServiceUnavailable(format!("修复租户授权版本失败: {error}")))
    }

    /// Outbox Worker 使用严格模式修复镜像；脚本只允许版本单调前进。
    pub async fn repair_user_version(
        &self,
        tenant_id: &str,
        user_id: i64,
        authorization_version: i32,
    ) -> AppResult<()> {
        let backend = self.backend.as_ref().ok_or_else(cache_unavailable)?;
        backend
            .update_user_version(tenant_id, user_id, authorization_version)
            .await
            .map_err(|error| AppError::ServiceUnavailable(format!("修复用户授权版本失败: {error}")))
    }

    pub async fn repair_namespace_version(
        &self,
        tenant_id: &str,
        namespace: &str,
        namespace_version: i64,
    ) -> AppResult<()> {
        validate_cache_namespace(namespace)?;
        validate_namespace_version(namespace_version)?;
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                // optional 模式关闭 Redis 时仍消费事务性 Outbox；以后启用 Redis 后，
                // 首次读取会从数据库权威版本恢复镜像。
                Ok(())
            };
        };
        backend
            .update_namespace_version(tenant_id, namespace, namespace_version)
            .await
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("修复租户缓存命名空间版本失败: {error}"))
            })
    }

    pub async fn read_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
    ) -> AppResult<Option<TenantCacheLookup>> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(None)
            };
        };
        match backend.read_tenant_value(tenant_id, namespace).await {
            Ok(value) => Ok(value),
            Err(error) if self.required => {
                tracing::error!(tenant_id, namespace, %error, "租户版本化缓存读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, namespace, %error, "租户版本化缓存读取失败，回源数据库");
                Ok(None)
            }
        }
    }

    pub async fn store_tenant_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        authorization_epoch: i32,
        value: &str,
        ttl_secs: u64,
    ) -> AppResult<bool> {
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(false)
            };
        };
        match backend
            .store_tenant_value(tenant_id, namespace, authorization_epoch, value, ttl_secs)
            .await
        {
            Ok(stored) => Ok(stored),
            Err(error) if self.required => {
                tracing::error!(tenant_id, namespace, %error, "租户版本化缓存写入失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, namespace, %error, "租户版本化缓存写入失败");
                Ok(false)
            }
        }
    }

    pub async fn read_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
    ) -> AppResult<Option<NamespaceCacheLookup>> {
        validate_cache_namespace(namespace)?;
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(None)
            };
        };
        match backend
            .read_namespace_value(tenant_id, namespace, item)
            .await
        {
            Ok(value) => Ok(value),
            Err(error) if self.required => {
                tracing::error!(tenant_id, namespace, item, %error, "独立租户缓存读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, namespace, item, %error, "独立租户缓存读取失败，回源数据库");
                Ok(None)
            }
        }
    }

    pub async fn store_namespace_value(
        &self,
        tenant_id: &str,
        namespace: &str,
        item: &str,
        namespace_version: i64,
        value: &str,
        ttl_secs: u64,
    ) -> AppResult<bool> {
        validate_cache_namespace(namespace)?;
        validate_namespace_version(namespace_version)?;
        let Some(backend) = &self.backend else {
            return if self.required {
                Err(cache_unavailable())
            } else {
                Ok(false)
            };
        };
        match backend
            .store_namespace_value(
                tenant_id,
                namespace,
                item,
                namespace_version,
                value,
                ttl_secs,
            )
            .await
        {
            Ok(stored) => Ok(stored),
            Err(error) if self.required => {
                tracing::error!(tenant_id, namespace, item, %error, "独立租户缓存写入失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, namespace, item, %error, "独立租户缓存写入失败");
                Ok(false)
            }
        }
    }

    fn handle_mirror_result(
        &self,
        result: Result<(), String>,
        tenant_id: &str,
        user_id: Option<i64>,
    ) -> AppResult<()> {
        match result {
            Ok(()) => Ok(()),
            Err(error) if self.required => {
                tracing::error!(tenant_id, ?user_id, %error, "授权版本镜像同步失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                tracing::warn!(tenant_id, ?user_id, %error, "授权版本镜像同步失败，等待 Outbox 修复");
                Ok(())
            }
        }
    }
}

struct RedisAuthorizationCacheBackend {
    redis: RedisClient,
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

fn cache_unavailable() -> AppError {
    AppError::ServiceUnavailable("授权缓存暂不可用，已拒绝本次权限敏感操作".into())
}

fn mirror_event(
    tenant_id: &str,
    aggregate_type: &str,
    aggregate_id: &str,
    version: i64,
    payload: AuthorizationMirrorUpdate,
    available_at: chrono::DateTime<chrono::Utc>,
) -> AppResult<RecordOutboxEvent> {
    let trace_context = crate::trace_context::current_trace_context();
    Ok(RecordOutboxEvent {
        tenant_id: Some(tenant_id.to_owned()),
        event_type: AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE.to_owned(),
        aggregate_type: aggregate_type.to_owned(),
        aggregate_id: aggregate_id.to_owned(),
        payload: serde_json::to_value(payload)
            .map_err(|error| AppError::Internal(format!("序列化授权镜像事件失败: {error}")))?,
        available_at,
        max_attempts: 20,
        dedupe_key: Some(format!(
            "authorization-mirror:{tenant_id}:{aggregate_type}:{aggregate_id}:{version}"
        )),
        traceparent: trace_context.traceparent,
        tracestate: trace_context.tracestate,
    })
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

fn validate_namespace_version(version: i64) -> AppResult<()> {
    if version < 0 {
        return Err(AppError::Database("缓存命名空间版本不能为负数".into()));
    }
    Ok(())
}

fn tenant_hash_tag(tenant_id: &str) -> String {
    format!("{{{tenant_id}}}")
}

fn tenant_epoch_key(tenant_id: &str) -> String {
    format!("ryframe:authorization:{}:epoch", tenant_hash_tag(tenant_id))
}

fn user_version_key(tenant_id: &str, user_id: i64) -> String {
    format!(
        "ryframe:authorization:{}:user:{user_id}:version",
        tenant_hash_tag(tenant_id)
    )
}

fn snapshot_hash_key(tenant_id: &str, user_id: i64) -> String {
    format!(
        "ryframe:authorization:{}:user:{user_id}:snapshots",
        tenant_hash_tag(tenant_id)
    )
}

#[cfg(test)]
fn snapshot_field(versions: AuthorizationVersions) -> String {
    format!(
        "{}:{}",
        versions.tenant_authorization_epoch, versions.user_authorization_version
    )
}

fn tenant_value_hash_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}",
        tenant_hash_tag(tenant_id)
    )
}

fn namespace_version_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}:version",
        tenant_hash_tag(tenant_id)
    )
}

fn namespace_values_hash_key(tenant_id: &str, namespace: &str) -> String {
    format!(
        "ryframe:tenant-cache:{}:{namespace}:values",
        tenant_hash_tag(tenant_id)
    )
}

#[cfg(test)]
mod tests {
    use ryframe_kernel::{ActorContext, DataScope};

    use super::*;

    fn snapshot() -> AuthorizationSnapshot {
        AuthorizationSnapshot {
            versions: AuthorizationVersions {
                tenant_authorization_epoch: 8,
                user_authorization_version: 13,
            },
            tenant_session_version: 3,
            principal: RequestPrincipal {
                actor: ActorContext {
                    user_id: 42,
                    tenant_id: "tenant-a".into(),
                    username: "alice".into(),
                    dept_id: Some(7),
                    dept_path: Some("0,1".into()),
                    data_scope: DataScope::Custom,
                    custom_dept_ids: vec![7, 9],
                    include_self: true,
                    is_super_admin: false,
                },
                preferred_locale: Some("zh-CN".into()),
                roles: vec!["auditor".into()],
                role_ids: vec![5],
                permissions: vec!["system:user:list".into()],
                tenant_request_limit_per_minute: 600,
            },
        }
    }

    #[test]
    fn snapshot_address_contains_both_authorization_versions() {
        let snapshot = snapshot();
        assert_eq!(
            format!(
                "{}#{}",
                snapshot_hash_key("tenant-a", 42),
                snapshot_field(snapshot.versions)
            ),
            "ryframe:authorization:{tenant-a}:user:42:snapshots#8:13"
        );
    }

    #[test]
    fn snapshot_round_trip_contains_roles_permissions_and_data_scope() {
        let snapshot = snapshot();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: AuthorizationSnapshot = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.principal.roles, vec!["auditor"]);
        assert_eq!(decoded.principal.permissions, vec!["system:user:list"]);
        assert_eq!(decoded.principal.actor.data_scope, DataScope::Custom);
        assert_eq!(decoded.principal.actor.dept_path.as_deref(), Some("0,1"));
        assert_eq!(decoded.principal.actor.custom_dept_ids, vec![7, 9]);
    }

    #[test]
    fn hot_read_script_fetches_versions_and_snapshot_in_one_lua_call() {
        assert!(READ_SNAPSHOT_SCRIPT.contains("redis.call('GET', KEYS[1])"));
        assert!(READ_SNAPSHOT_SCRIPT.contains("redis.call('GET', KEYS[2])"));
        assert!(READ_SNAPSHOT_SCRIPT.contains("KEYS[3], tenant_epoch .. ':' .. user_version"));
        assert_eq!(READ_SNAPSHOT_SCRIPT.matches("redis.call").count(), 3);
    }

    #[test]
    fn redis_cluster_keys_share_one_tenant_hash_slot() {
        let keys = [
            tenant_epoch_key("tenant-a"),
            user_version_key("tenant-a", 42),
            snapshot_hash_key("tenant-a", 42),
        ];
        assert!(keys.iter().all(|key| key.contains("{tenant-a}")));
        assert!(!READ_SNAPSHOT_SCRIPT.contains("ARGV["));

        let tenant_cache_keys = [
            tenant_epoch_key("tenant-a"),
            tenant_value_hash_key("tenant-a", "menu-tree"),
        ];
        assert!(
            tenant_cache_keys
                .iter()
                .all(|key| key.contains("{tenant-a}"))
        );
        assert!(!READ_TENANT_VALUE_SCRIPT.contains("ARGV["));

        let namespace_cache_keys = [
            namespace_version_key("tenant-a", "config"),
            namespace_values_hash_key("tenant-a", "config"),
        ];
        assert!(
            namespace_cache_keys
                .iter()
                .all(|key| key.contains("{tenant-a}"))
        );
        assert!(READ_NAMESPACE_VALUE_SCRIPT.contains("ARGV[1]"));
    }

    #[test]
    fn mirror_repair_script_never_moves_a_version_backwards() {
        assert!(UPDATE_MIRROR_SCRIPT.contains("tonumber(current) > incoming"));
        assert!(UPDATE_MIRROR_SCRIPT.contains("redis.call('SET', KEYS[1], ARGV[1])"));
    }

    #[test]
    fn namespace_version_uses_exact_decimal_comparison_without_lua_numbers() {
        assert!(!ADVANCE_NAMESPACE_VERSION_SCRIPT.contains("tonumber"));
        assert!(ADVANCE_NAMESPACE_VERSION_SCRIPT.contains("string.len(left)"));
        assert!(ADVANCE_NAMESPACE_VERSION_SCRIPT.contains("left < right"));
        assert!(ADVANCE_NAMESPACE_VERSION_SCRIPT.contains("[^0-9]"));
        assert!(
            ADVANCE_NAMESPACE_VERSION_SCRIPT.contains("compare_decimal(incoming, current) <= 0")
        );
    }

    #[test]
    fn namespace_hash_is_cleared_only_when_the_version_advances() {
        let compare_at = ADVANCE_NAMESPACE_VERSION_SCRIPT
            .find("compare_decimal(incoming, current) <= 0")
            .unwrap();
        let delete_at = ADVANCE_NAMESPACE_VERSION_SCRIPT
            .find("redis.call('DEL', KEYS[2])")
            .unwrap();
        assert!(compare_at < delete_at);
        assert_eq!(ADVANCE_NAMESPACE_VERSION_SCRIPT.matches("DEL").count(), 1);
        assert!(!WRITE_NAMESPACE_VALUE_SCRIPT.contains("DEL"));
        assert!(WRITE_NAMESPACE_VALUE_SCRIPT.contains("HSET', KEYS[2], ARGV[2], ARGV[4]"));
    }

    #[test]
    fn missing_redis_version_is_reported_without_inventing_zero() {
        assert!(READ_NAMESPACE_VALUE_SCRIPT.contains("return {false, false}"));
        assert!(!READ_NAMESPACE_VALUE_SCRIPT.contains("SET"));
        assert!(!READ_NAMESPACE_VALUE_SCRIPT.contains("DEL"));
    }
}
