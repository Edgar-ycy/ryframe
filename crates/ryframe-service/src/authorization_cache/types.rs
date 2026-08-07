use async_trait::async_trait;
use ryframe_auth::RequestPrincipal;
use serde::{Deserialize, Serialize};

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
    pub(super) fn miss() -> Self {
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

/// 权限缓存后端接口；当前实现使用 Redis，并保留后端替换边界。
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
