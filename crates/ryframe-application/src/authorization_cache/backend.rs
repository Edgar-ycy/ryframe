use std::{fmt, sync::Arc};

use ryframe_kernel::{AppError, AppResult};

use crate::CacheAvailabilityPolicy;

use super::*;

#[derive(Clone)]
pub struct AuthorizationCache {
    backend: Option<Arc<dyn AuthorizationCacheBackend>>,
    publisher: Option<Arc<dyn AuthorizationChangePublisher>>,
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
    pub fn new(
        backend: Option<Arc<dyn AuthorizationCacheBackend>>,
        publisher: Option<Arc<dyn AuthorizationChangePublisher>>,
        policy: CacheAvailabilityPolicy,
    ) -> Self {
        Self {
            backend,
            publisher,
            required: policy.is_required(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            backend: None,
            publisher: None,
            required: false,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.backend.is_some()
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    pub async fn lookup_snapshot(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<AuthorizationCacheLookup> {
        let Some(backend) = &self.backend else {
            return if self.required {
                record_authorization_cache_lookup("snapshot", "error");
                Err(cache_unavailable())
            } else {
                record_authorization_cache_lookup("snapshot", "bypass");
                Ok(AuthorizationCacheLookup::miss())
            };
        };
        match backend.lookup_snapshot(tenant_id, user_id).await {
            Ok(lookup) => {
                record_authorization_cache_lookup(
                    "snapshot",
                    if lookup.snapshot.is_some() {
                        "hit"
                    } else {
                        "miss"
                    },
                );
                Ok(lookup)
            }
            Err(error) if self.required => {
                record_authorization_cache_lookup("snapshot", "error");
                tracing::error!(tenant_id, user_id, %error, "授权快照原子读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                record_authorization_cache_lookup("snapshot", "fallback");
                tracing::warn!(tenant_id, user_id, %error, "授权快照读取失败，回源主库");
                Ok(AuthorizationCacheLookup::miss())
            }
        }
    }

    /// 为只读诊断返回 Redis 的原始版本镜像状态。
    ///
    /// 该入口不会执行 optional 模式的主库回退，也不会把错误伪装成缓存未命中；调用方
    /// 只能展示版本号，不能返回完整授权快照。
    pub async fn inspect_snapshot(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> Result<Option<AuthorizationCacheLookup>, String> {
        let Some(backend) = &self.backend else {
            return Ok(None);
        };
        backend.lookup_snapshot(tenant_id, user_id).await.map(Some)
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
        let mirror_result = backend
            .update_tenant_epoch(tenant_id, authorization_epoch)
            .await;
        let mirror_updated = mirror_result.is_ok();
        self.handle_mirror_result(mirror_result, tenant_id, None)?;
        if mirror_updated {
            self.publish_tenant_context_changed(tenant_id, authorization_epoch)
                .await;
        }
        Ok(())
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
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("修复租户授权版本失败: {error}"))
            })?;
        self.publish_tenant_context_changed(tenant_id, authorization_epoch)
            .await;
        Ok(())
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
                record_authorization_cache_lookup("tenant", "error");
                Err(cache_unavailable())
            } else {
                record_authorization_cache_lookup("tenant", "bypass");
                Ok(None)
            };
        };
        match backend.read_tenant_value(tenant_id, namespace).await {
            Ok(value) => {
                record_authorization_cache_lookup(
                    "tenant",
                    if value.as_ref().is_some_and(|lookup| lookup.value.is_some()) {
                        "hit"
                    } else {
                        "miss"
                    },
                );
                Ok(value)
            }
            Err(error) if self.required => {
                record_authorization_cache_lookup("tenant", "error");
                tracing::error!(tenant_id, namespace, %error, "租户版本化缓存读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                record_authorization_cache_lookup("tenant", "fallback");
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
                record_authorization_cache_lookup("namespace", "error");
                Err(cache_unavailable())
            } else {
                record_authorization_cache_lookup("namespace", "bypass");
                Ok(None)
            };
        };
        match backend
            .read_namespace_value(tenant_id, namespace, item)
            .await
        {
            Ok(value) => {
                record_authorization_cache_lookup(
                    "namespace",
                    if value.as_ref().is_some_and(|lookup| lookup.value.is_some()) {
                        "hit"
                    } else {
                        "miss"
                    },
                );
                Ok(value)
            }
            Err(error) if self.required => {
                record_authorization_cache_lookup("namespace", "error");
                tracing::error!(tenant_id, namespace, item, %error, "独立租户缓存读取失败");
                Err(cache_unavailable())
            }
            Err(error) => {
                record_authorization_cache_lookup("namespace", "fallback");
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

    /// 发布租户上下文已变化的跨实例加速信号。事件不携带权威快照，
    /// API 订阅者必须回控制库强一致读取四值后再通知浏览器。
    pub async fn publish_tenant_context_changed(&self, tenant_id: &str, authorization_epoch: i32) {
        let Some(publisher) = &self.publisher else {
            return;
        };
        let event = AuthorizationChangedEvent {
            tenant_id: tenant_id.to_owned(),
            authorization_epoch,
        };
        let payload = match serde_json::to_string(&event) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(tenant_id, authorization_epoch, %error, "授权变化事件序列化失败");
                return;
            }
        };
        if let Err(error) = publisher
            .publish(AUTHORIZATION_CHANGED_REDIS_CHANNEL, &payload)
            .await
        {
            // 实时通知是界面加速通道；授权快照和后续响应头仍负责最终一致性。
            tracing::warn!(tenant_id, authorization_epoch, %error, "授权变化实时通知发布失败");
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

fn cache_unavailable() -> AppError {
    AppError::ServiceUnavailable("授权缓存暂不可用，已拒绝本次权限敏感操作".into())
}

fn validate_namespace_version(version: i64) -> AppResult<()> {
    if version < 0 {
        return Err(AppError::Database("缓存命名空间版本不能为负数".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_cache_namespace;

    #[test]
    fn cache_namespace_rejects_unsafe_key_fragments() {
        assert!(validate_cache_namespace("config").is_ok());
        assert!(validate_cache_namespace("tenant.cache-v1").is_ok());
        assert!(validate_cache_namespace("").is_err());
        assert!(validate_cache_namespace("Config").is_err());
        assert!(validate_cache_namespace("../config").is_err());
    }
}
