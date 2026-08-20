use std::sync::OnceLock;
mod backend;
mod event;
mod mirror;
mod types;

pub use backend::AuthorizationCache;
pub use event::{
    AUTHORIZATION_CHANGED_REDIS_CHANNEL, AuthorizationChangePublishFuture,
    AuthorizationChangePublisher, AuthorizationChangedEvent,
};
pub use types::{
    AuthorizationCacheBackend, AuthorizationCacheLookup, AuthorizationMirrorUpdate,
    AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
};

pub const AUTHORIZATION_SNAPSHOT_TTL_SECS: u64 = 300;
pub const AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE: &str = "security.authorization.mirror-updated";

fn validate_cache_namespace(namespace: &str) -> ryframe_kernel::AppResult<()> {
    if namespace.is_empty()
        || namespace.len() > 64
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(ryframe_kernel::AppError::Validation(
            "缓存命名空间只能包含 1 到 64 个小写字母、数字、点、下划线或连字符".into(),
        ));
    }
    Ok(())
}

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
