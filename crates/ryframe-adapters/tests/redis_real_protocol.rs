use std::{
    env, process,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use ryframe_adapters::{
    RedisClient,
    idempotency::{
        RedisIdempotencyStore, RemoteIdempotencyReservation, idempotency_guard_key,
        idempotency_meta_key, idempotency_response_key,
    },
};
use ryframe_config::{AppConfig, Environment, RedisConfig, RedisMode};

#[path = "redis_real_protocol/transactions.rs"]
mod transactions;

const ENABLE_ENV: &str = "RYFRAME_REDIS_INTEGRATION";
const TLS_ENABLE_ENV: &str = "RYFRAME_REDIS_TLS_INTEGRATION";
static SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[tokio::test]
async fn tls_connection_round_trip_uses_real_redis() {
    if !integration_enabled(TLS_ENABLE_ENV) {
        eprintln!("跳过 Redis TLS 真实协议测试：设置 {TLS_ENABLE_ENV}=1 后才会连接外部服务");
        return;
    }

    let config = redis_config("tls");
    assert!(config.tls, "Redis TLS 集成测试必须设置 RYFRAME_REDIS_TLS=1");
    let client = RedisClient::connect(&config)
        .await
        .unwrap_or_else(|error| panic!("使用 TLS 连接 Redis 真实服务失败: {error}"));
    let key = "protocol:tls";
    let result = async {
        client.set(key, "ready").await.map_err(redis_error)?;
        let value = client.get(key).await.map_err(redis_error)?;
        if value.as_deref() == Some("ready") {
            Ok(())
        } else {
            Err(format!("Redis TLS 往返结果不匹配: {value:?}"))
        }
    }
    .await;
    finish_with_cleanup(result, &[(&client, vec![key.to_owned()])]).await;
}

#[tokio::test]
async fn redis_client_applies_and_expires_ttl() {
    if !integration_enabled(ENABLE_ENV) {
        skip_message();
        return;
    }

    let client = connect_client("ttl").await;
    let key = "protocol:ttl";
    let result = async {
        client.set_ex(key, "ready", 2).await.map_err(redis_error)?;
        let ttl = client.ttl(key).await.map_err(redis_error)?;
        if !(1..=2).contains(&ttl) {
            return Err(format!("Redis TTL 应在 1 到 2 秒之间，实际为 {ttl}"));
        }
        tokio::time::sleep(Duration::from_millis(2_200)).await;
        let value = client.get(key).await.map_err(redis_error)?;
        if value.is_some() {
            return Err("Redis SET EX 键在 TTL 后仍然存在".to_owned());
        }
        Ok(())
    }
    .await;
    finish_with_cleanup(result, &[(&client, vec![key.to_owned()])]).await;
}

#[tokio::test]
async fn idempotency_reservation_is_atomic_and_replayable() {
    if !integration_enabled(ENABLE_ENV) {
        skip_message();
        return;
    }

    let client = connect_client("idempotency").await;
    let store = RedisIdempotencyStore::new(client.clone());
    let other_store = store.clone();
    let key = "same-request";
    let fingerprint = "sha256:request-a";
    let result = async {
        let (first, second) = tokio::join!(
            store.reserve(key, fingerprint, 30),
            other_store.reserve(key, fingerprint, 30),
        );
        let reservations = [first.map_err(identity)?, second.map_err(identity)?];
        let acquired = reservations
            .iter()
            .filter(|reservation| matches!(reservation, RemoteIdempotencyReservation::Acquired))
            .count();
        let processing = reservations
            .iter()
            .filter(|reservation| matches!(reservation, RemoteIdempotencyReservation::Processing))
            .count();
        if acquired != 1 || processing != 1 {
            return Err(format!(
                "并发幂等保留应为一次获取、一次处理中，实际为 {reservations:?}"
            ));
        }

        store
            .begin_execution(key, fingerprint, 30)
            .await
            .map_err(identity)?;
        store
            .complete(key, fingerprint, r#"{"status":"ok"}"#, 30)
            .await
            .map_err(identity)?;
        let replay = store
            .reserve(key, fingerprint, 30)
            .await
            .map_err(identity)?;
        if replay != RemoteIdempotencyReservation::Completed(r#"{"status":"ok"}"#.to_owned()) {
            return Err(format!("幂等完成结果无法重放: {replay:?}"));
        }
        let conflict = store
            .reserve(key, "sha256:different-request", 30)
            .await
            .map_err(identity)?;
        if conflict != RemoteIdempotencyReservation::Conflict {
            return Err(format!("不同请求指纹未产生幂等冲突: {conflict:?}"));
        }
        Ok(())
    }
    .await;

    let keys = vec![
        idempotency_meta_key(key),
        idempotency_response_key(key),
        idempotency_guard_key(key),
    ];
    finish_with_cleanup(result, &[(&client, keys)]).await;
}

#[tokio::test]
async fn redis_clients_cannot_cross_scope_namespaces() {
    if !integration_enabled(ENABLE_ENV) {
        skip_message();
        return;
    }

    let first = connect_client("namespace-a").await;
    let second = connect_client("namespace-b").await;
    let key = "protocol:same-logical-key";
    let result = async {
        if first.namespace() == second.namespace() {
            return Err("两个集成测试 Redis 客户端意外共享 namespace".to_owned());
        }
        first.set(key, "first").await.map_err(redis_error)?;
        second.set(key, "second").await.map_err(redis_error)?;
        let first_value = first.get(key).await.map_err(redis_error)?;
        let second_value = second.get(key).await.map_err(redis_error)?;
        if first_value.as_deref() != Some("first") || second_value.as_deref() != Some("second") {
            return Err(format!(
                "Redis namespace 隔离失败: first={first_value:?}, second={second_value:?}"
            ));
        }
        if first.scoped_key(key) == second.scoped_key(key) {
            return Err("相同逻辑键被映射为相同物理 Redis 键".to_owned());
        }
        Ok(())
    }
    .await;

    finish_with_cleanup(
        result,
        &[
            (&first, vec![key.to_owned()]),
            (&second, vec![key.to_owned()]),
        ],
    )
    .await;
}

#[tokio::test]
async fn unavailable_redis_returns_explicit_connection_error() {
    if !integration_enabled(ENABLE_ENV) {
        skip_message();
        return;
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("分配不可用 Redis 测试端口失败: {error}"));
    let port = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("读取不可用 Redis 测试端口失败: {error}"))
        .port();
    drop(listener);

    let mut config = redis_config("unavailable");
    config.host = "127.0.0.1".to_owned();
    config.port = port;
    config.timeout_secs = 1;
    if RedisClient::connect(&config).await.is_ok() {
        panic!("Redis 服务不可用时连接意外成功");
    }
}

async fn connect_client(test_name: &str) -> RedisClient {
    let config = redis_config(test_name);
    RedisClient::connect(&config)
        .await
        .unwrap_or_else(|error| panic!("连接 Redis 真实服务失败: {error}"))
}

fn redis_config(test_name: &str) -> RedisConfig {
    let scope = unique_scope(test_name);
    let config_source = include_str!("../../../config/app.toml").replacen(
        "scope_id = \"dev-local\"",
        &format!("scope_id = \"{scope}\""),
        1,
    );
    let config_dir = tempfile::tempdir()
        .unwrap_or_else(|error| panic!("创建 Redis 集成测试配置目录失败: {error}"));
    std::fs::write(config_dir.path().join("app.toml"), config_source)
        .unwrap_or_else(|error| panic!("写入 Redis 集成测试配置失败: {error}"));
    let mut app = AppConfig::load(config_dir.path(), Environment::Test)
        .unwrap_or_else(|error| panic!("加载 Redis 集成测试配置失败: {error}"));
    let mut redis = app
        .redis
        .take()
        .unwrap_or_else(|| panic!("基础配置缺少 Redis 配置"));
    redis.mode = RedisMode::Required;
    redis.host = env_value("RYFRAME_REDIS_HOST", "127.0.0.1");
    redis.port = env_parse("RYFRAME_REDIS_PORT", 6379);
    redis.password = env_value("RYFRAME_REDIS_PASSWORD", "");
    redis.database = env_parse("RYFRAME_REDIS_DATABASE", 0);
    redis.timeout_secs = 3;
    redis.tls = env_switch("RYFRAME_REDIS_TLS");
    redis.tls_ca = env_optional("RYFRAME_REDIS_TLS_CA");
    redis.tls_client_cert = env_optional("RYFRAME_REDIS_TLS_CLIENT_CERT");
    redis.tls_client_key = env_optional("RYFRAME_REDIS_TLS_CLIENT_KEY");
    redis
}

async fn finish_with_cleanup(result: Result<(), String>, cleanup: &[(&RedisClient, Vec<String>)]) {
    let mut cleanup_errors = Vec::new();
    for (client, keys) in cleanup {
        for key in keys {
            if let Err(error) = client.del(key).await {
                cleanup_errors.push(format!(
                    "精确清理 Redis 键 {} 失败: {error}",
                    client.scoped_key(key)
                ));
            }
        }
    }
    if !cleanup_errors.is_empty() {
        panic!("{}", cleanup_errors.join("; "));
    }
    if let Err(error) = result {
        panic!("{error}");
    }
}

fn unique_scope(test_name: &str) -> String {
    let run = sanitized_fragment(&env_value("RYFRAME_INTEGRATION_RUN_ID", "local"), 12);
    let test = sanitized_fragment(test_name, 12);
    let sequence = SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("it-{run}-{test}-{}-{sequence}", process::id())
}

fn sanitized_fragment(value: &str, max_len: usize) -> String {
    value
        .chars()
        .filter_map(|character| {
            let character = character.to_ascii_lowercase();
            character.is_ascii_alphanumeric().then_some(character)
        })
        .take(max_len)
        .collect::<String>()
}

fn integration_enabled(name: &str) -> bool {
    match env::var(name) {
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{name} 只接受精确值 1，当前值为 {value:?}"),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

fn skip_message() {
    eprintln!("跳过 Redis 真实协议测试：设置 {ENABLE_ENV}=1 后才会连接外部服务");
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn env_optional(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        Ok(_) | Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

fn env_switch(name: &str) -> bool {
    match env::var(name) {
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{name} 只接受精确值 1，当前值为 {value:?}"),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

fn env_parse<T>(name: &str, default: T) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|error| panic!("{name} 解析失败: {error}")),
        Err(env::VarError::NotPresent) => default,
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}

fn redis_error(error: redis::RedisError) -> String {
    error.to_string()
}

fn identity(error: String) -> String {
    error
}
