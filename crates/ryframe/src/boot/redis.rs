use ryframe_config::RedisConfig;
use ryframe_core::{RedisClient, TokenBlacklist, create_redis_client};

/// Redis 初始化结果
pub struct RedisState {
    pub client: Option<RedisClient>,
    pub token_blacklist: TokenBlacklist,
}

/// 初始化 Redis 客户端 + Token 黑名单
///
/// Redis 不可用时自动降级为内存模式。
pub async fn init(config: &Option<RedisConfig>) -> RedisState {
    let redis_client = create_redis_client(config).await;
    if redis_client.is_some() {
        tracing::info!(
            "Redis 已启用，验证码/在线用户/限流器/菜单树/部门树/字典缓存将使用 Redis 存储"
        );
    } else {
        tracing::info!("Redis 未配置或不可用，使用内存模式");
    }

    let token_blacklist = TokenBlacklist::new(redis_client.clone());
    if redis_client.is_none() {
        token_blacklist.spawn_gc(); // 内存模式需要后台 GC
    }

    RedisState {
        client: redis_client,
        token_blacklist,
    }
}
