use crate::RedisClient;

use super::{Cache, CacheError, backend::RedisCache};

/// Default TTL for a user's resolved API permission codes.
pub const USER_PERMISSION_CACHE_TTL_SECS: u64 = 30 * 60;

/// Build a tenant-scoped permission cache key.
pub fn user_permission_cache_key(tenant_id: &str, user_id: i64) -> String {
    format!("user:perms:{tenant_id}:{user_id}")
}

/// Read a user's resolved API permission codes from Redis.
pub async fn get_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
) -> Result<Option<Vec<String>>, CacheError> {
    RedisCache::new(redis.clone())
        .get(&user_permission_cache_key(tenant_id, user_id))
        .await
}

/// Cache a user's resolved API permission codes in Redis.
pub async fn set_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
    permissions: &[String],
) -> Result<(), CacheError> {
    RedisCache::new(redis.clone())
        .set(
            &user_permission_cache_key(tenant_id, user_id),
            &permissions,
            USER_PERMISSION_CACHE_TTL_SECS,
        )
        .await
}

/// Delete one user's cached API permission codes.
pub async fn clear_user_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
    user_id: i64,
) -> Result<(), CacheError> {
    RedisCache::new(redis.clone())
        .delete(&user_permission_cache_key(tenant_id, user_id))
        .await
}

/// Delete all cached API permission codes for one tenant.
pub async fn clear_tenant_permission_cache(
    redis: &RedisClient,
    tenant_id: &str,
) -> Result<u64, CacheError> {
    RedisCache::new(redis.clone())
        .delete_by_prefix(&format!("user:perms:{tenant_id}:"))
        .await
}
