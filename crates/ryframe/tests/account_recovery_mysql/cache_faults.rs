use std::sync::Arc;

use futures_util::FutureExt;
use redis::AsyncCommands;
use ryframe_adapters::RedisClient;
use ryframe_api::AppState;
use ryframe_application::{
    AuthService, AuthorizationCache, CacheAvailabilityPolicy, LoginResult, PrincipalResolver,
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_config::RedisConfig;
use ryframe_kernel::AppError;
use sea_orm::DatabaseConnection;

use super::{
    account_http::Server, cleanup_authorization_keys, message, redis_configuration,
    role_assignment, services, services_with_cache,
};

#[path = "cache_faults/transport.rs"]
mod transport;

pub(super) async fn verify(
    database: &DatabaseConnection,
    redis: &redis_configuration::ScopedClient<RedisClient>,
    cache: &AuthorizationCache,
    auth: Arc<AuthService>,
    session: &LoginResult,
) -> Result<(), String> {
    let claims = ryframe_auth::jwt::decode_token(&session.access_token, &auth.token_settings())
        .map_err(message)?;
    let (_, authoritative) = services(database.clone())?;
    let expected = authoritative
        .resolve_principal(&claims)
        .await
        .map_err(message)?;
    let (state, lifetime) = role_assignment::state(database).await?;
    let result = std::panic::AssertUnwindSafe(async {
        redis.configuration.verify()?;
        let server = profile_server(state.clone(), auth.clone()).await?;
        missing_and_ahead(
            redis,
            cache,
            &auth,
            &claims,
            &session.access_token,
            &server,
            &expected,
        )
        .await?;
        network_recovery(database, redis, &state, &expected).await?;
        redis.configuration.verify()
    })
    .catch_unwind()
    .await;
    drop(state);
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            lifetime.preserve();
            Err(error)
        }
        Err(payload) => {
            lifetime.preserve();
            std::panic::resume_unwind(payload)
        }
    }
}

async fn profile_server(mut state: AppState, auth: Arc<AuthService>) -> Result<Server, String> {
    state.auth.token_settings = auth.token_settings();
    state.auth.refresh_sessions =
        ryframe::boot::session_security::refresh_sessions(auth.refresh_sessions());
    state.auth.principal_resolver = auth;
    Server::start(state).await
}

async fn missing_and_ahead(
    redis: &RedisClient,
    cache: &AuthorizationCache,
    auth: &AuthService,
    claims: &Claims,
    token: &str,
    server: &Server,
    expected: &RequestPrincipal,
) -> Result<(), String> {
    cleanup_authorization_keys(redis, &[2]).await?;
    let missing = cache
        .inspect_snapshot("system", 2)
        .await
        .map_err(|_| "读取真实 Redis missing 状态失败")?
        .ok_or("真实缓存未启用")?;
    assert!(missing.tenant_authorization_epoch.is_none());
    assert!(missing.user_authorization_version.is_none());
    assert!(missing.snapshot.is_none());
    println!("→ 授权缓存 missing 已直接观测，核验同一有效会话回源重建");
    // HTTP 必须通过真实 resolver 回源并重建；同一个 token 的刷新会话没有被撤销。
    require_profile(server, token, 200).await?;
    same_principal(
        &auth.resolve_principal(claims).await.map_err(message)?,
        expected,
    )?;
    require_authoritative_cache(cache, expected).await?;

    let key = redis
        .scoped_key(&ryframe::boot::authorization_cache_keyspace::user_version_key("system", 2));
    let ahead = claims
        .user_authorization_version
        .checked_add(1)
        .ok_or("授权测试版本溢出")?;
    let mut connection = redis.conn().clone();
    let _: () = connection
        .set(&key, ahead)
        .await
        .map_err(|_| "建立本 scope ahead 状态失败")?;
    let observed = cache
        .inspect_snapshot("system", 2)
        .await
        .map_err(|_| "读取真实 Redis ahead 状态失败")?
        .ok_or("真实缓存未启用")?;
    assert_eq!(observed.user_authorization_version, Some(ahead));
    println!("→ 授权缓存 ahead 已直接观测，核验 resolver 与 HTTP 拒绝");
    assert!(matches!(
        auth.resolve_principal(claims).await,
        Err(AppError::Authentication(_))
    ));
    require_profile(server, token, 401).await?;

    // 仅删除本用例已登记主体的授权键，再从主库恢复；不写低版本镜像冒充修复。
    cleanup_authorization_keys(redis, &[2]).await?;
    require_profile(server, token, 200).await?;
    same_principal(
        &auth.resolve_principal(claims).await.map_err(message)?,
        expected,
    )?;
    require_authoritative_cache(cache, expected).await
}

async fn require_authoritative_cache(
    cache: &AuthorizationCache,
    expected: &RequestPrincipal,
) -> Result<(), String> {
    let observed = cache
        .inspect_snapshot("system", 2)
        .await
        .map_err(|_| "读取恢复后的真实授权缓存失败")?
        .ok_or("真实缓存未启用")?;
    assert_eq!(
        observed.tenant_authorization_epoch,
        Some(expected.tenant_authorization_epoch)
    );
    assert_eq!(
        observed.user_authorization_version,
        Some(expected.user_authorization_version)
    );
    let snapshot = observed.snapshot.ok_or("真实 Redis 未重建授权快照")?;
    assert_eq!(
        snapshot.versions.data_scope_version,
        expected.data_scope_version
    );
    same_principal(&snapshot.principal, expected)
}

fn same_principal(actual: &RequestPrincipal, expected: &RequestPrincipal) -> Result<(), String> {
    // 主体不含 token 或凭据；比较完整身份、权限、数据范围及版本，避免只看 HTTP 200。
    assert_eq!(
        serde_json::to_value(actual).map_err(message)?,
        serde_json::to_value(expected).map_err(message)?
    );
    Ok(())
}

async fn require_profile(server: &Server, token: &str, status: u16) -> Result<(), String> {
    let response = server.get("/api/v1/auth/profile", token).await?;
    assert_eq!(response.status, status);
    match status {
        200 => assert_eq!(response.body["data"]["username"], "user"),
        401 => assert_eq!(response.body["error_key"], "authentication"),
        503 => assert_eq!(response.body["error_key"], "service_unavailable"),
        _ => return Err("授权缓存用例使用了未登记的 HTTP 预期状态".into()),
    }
    Ok(())
}

struct Session {
    auth: Arc<AuthService>,
    claims: Claims,
    token: String,
    cache: AuthorizationCache,
    server: Server,
}

impl Session {
    async fn new(
        database: &DatabaseConnection,
        state: &AppState,
        client: &RedisClient,
        policy: CacheAvailabilityPolicy,
        expected: &RequestPrincipal,
    ) -> Result<Self, String> {
        let cache = ryframe::boot::authorization_cache::cache(Some(client.clone()), policy);
        let (_, auth) = services_with_cache(database.clone(), cache.clone())?;
        let auth = Arc::new(auth);
        // 只登录已完成改密的同一用户，不重做首次改密或密码重置。
        let session = auth
            .login("system", "user", "FirstLogin123!")
            .await
            .map_err(message)?;
        let claims = ryframe_auth::jwt::decode_token(&session.access_token, &auth.token_settings())
            .map_err(message)?;
        same_principal(
            &auth.resolve_principal(&claims).await.map_err(message)?,
            expected,
        )?;
        let server = profile_server(state.clone(), auth.clone()).await?;
        require_profile(&server, &session.access_token, 200).await?;
        Ok(Self {
            auth,
            claims,
            token: session.access_token,
            cache,
            server,
        })
    }

    async fn require_current(&self, expected: &RequestPrincipal) -> Result<(), String> {
        same_principal(
            &self
                .auth
                .resolve_principal(&self.claims)
                .await
                .map_err(message)?,
            expected,
        )?;
        require_profile(&self.server, &self.token, 200).await
    }
}

async fn network_recovery(
    database: &DatabaseConnection,
    direct: &redis_configuration::ScopedClient<RedisClient>,
    state: &AppState,
    expected: &RequestPrincipal,
) -> Result<(), String> {
    let original: &RedisConfig = direct
        .configuration
        .app
        .redis
        .as_ref()
        .ok_or("缺少绑定 Redis 配置")?;
    let mut proxy = transport::Proxy::start(original).await?;
    let config = proxy.configuration(original);
    let client = RedisClient::connect(&config)
        .await
        .map_err(|_| "透明代理不能保持原 Redis/TLS 配置连接；不降级 TLS 或重写主机名")?;
    assert_eq!(
        client
            .ping()
            .await
            .map_err(|_| "代理初始化真实 PING 失败")?,
        "PONG"
    );
    let required = Session::new(
        database,
        state,
        &client,
        CacheAvailabilityPolicy::Required,
        expected,
    )
    .await?;
    let optional = Session::new(
        database,
        state,
        &client,
        CacheAvailabilityPolicy::Optional,
        expected,
    )
    .await?;
    proxy.disconnect().await?;
    println!("→ 独占代理已关闭全部连接与 listener，核验真实网络失败及两种缓存政策");
    require_outage(direct, &client, &required, &optional, expected).await?;
    proxy.restore().await?;
    println!("→ 独占代理恢复原端口，等待原客户端与原会话恢复");
    transport::require_reconnected(&client, config.timeout_secs).await?;
    // 相同 client、AuthService、claims 和 token 恢复，不重新登录或替换客户端。
    required.require_current(expected).await?;
    optional.require_current(expected).await?;
    require_authoritative_cache(&required.cache, expected).await?;
    proxy.disconnect().await
}

async fn require_outage(
    direct: &RedisClient,
    client: &RedisClient,
    required: &Session,
    optional: &Session,
    expected: &RequestPrincipal,
) -> Result<(), String> {
    assert!(
        client.ping().await.is_err(),
        "必须观察到原客户端真实网络错误"
    );
    assert_eq!(
        direct
            .ping()
            .await
            .map_err(|_| "真实 Redis 本体不应被代理故障停止")?,
        "PONG"
    );
    assert!(required.cache.inspect_snapshot("system", 2).await.is_err());
    assert!(optional.cache.inspect_snapshot("system", 2).await.is_err());
    assert!(matches!(
        required.auth.resolve_principal(&required.claims).await,
        Err(AppError::ServiceUnavailable(_))
    ));
    require_profile(&required.server, &required.token, 503).await?;
    optional.require_current(expected).await
}
