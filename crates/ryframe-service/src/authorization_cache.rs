use std::sync::OnceLock;
mod backend;
mod keyspace;
mod mirror;
mod redis_backend;
mod types;

pub use backend::AuthorizationCache;
pub use types::{
    AuthorizationCacheBackend, AuthorizationCacheLookup, AuthorizationMirrorUpdate,
    AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
};

pub const AUTHORIZATION_SNAPSHOT_TTL_SECS: u64 = 300;
pub const AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE: &str = "security.authorization.mirror-updated";

static AUTHORIZATION_CACHE_LOOKUP_HOOK: OnceLock<fn(&'static str, &'static str)> = OnceLock::new();

/// 安装授权缓存查询指标钩子；重复安装不会覆盖已生效的钩子。
pub fn set_authorization_cache_lookup_hook(hook: fn(&'static str, &'static str)) {
    let _ = AUTHORIZATION_CACHE_LOOKUP_HOOK.set(hook);
}

fn record_authorization_cache_lookup(scope: &'static str, result: &'static str) {
    if let Some(hook) = AUTHORIZATION_CACHE_LOOKUP_HOOK.get() {
        hook(scope, result);
    }
}
