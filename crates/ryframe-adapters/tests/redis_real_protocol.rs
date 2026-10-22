use std::time::Duration;

use ryframe_adapters::{
    RedisClient,
    idempotency::{
        RedisIdempotencyStore, RemoteIdempotencyReservation, idempotency_guard_key,
        idempotency_meta_key, idempotency_response_key,
    },
};
use ryframe_config::{RedisConfig, RedisMode};

#[path = "support/configuration.rs"]
mod configuration;
use configuration::{Configuration, ScopedClient};

#[path = "redis_real_protocol/transactions.rs"]
mod transactions;

const GENERAL: &str = "redis";
const TLS: &str = "redis-tls";

#[tokio::test]
async fn tls_connection_round_trip_uses_real_redis() {
    if !integration_enabled(TLS) {
        skip_message();
        return;
    }

    let (config, snapshot) = role_config(TLS, "tls");
    assert!(config.tls, "Redis TLS 集成配置必须设置 redis.tls=true");
    let client = RedisClient::connect(&config)
        .await
        .unwrap_or_else(|error| panic!("使用 TLS 连接 Redis 真实服务失败: {error}"));
    let client = ScopedClient {
        client,
        configuration: snapshot,
    };
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
    if !integration_enabled(GENERAL) {
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
    if !integration_enabled(GENERAL) {
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
    if !integration_enabled(GENERAL) {
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
    if !integration_enabled(GENERAL) {
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

    let (mut config, _snapshot) = redis_config("unavailable");
    config.host = "127.0.0.1".to_owned();
    config.port = port;
    config.timeout_secs = 1;
    if RedisClient::connect(&config).await.is_ok() {
        panic!("Redis 服务不可用时连接意外成功");
    }
}

async fn connect_client(test_name: &str) -> ScopedClient<RedisClient> {
    let (config, snapshot) = redis_config(test_name);
    let client = RedisClient::connect(&config)
        .await
        .unwrap_or_else(|error| panic!("连接 Redis 真实服务失败: {error}"));
    ScopedClient {
        client,
        configuration: snapshot,
    }
}

fn redis_config(test_name: &str) -> (RedisConfig, Configuration) {
    role_config(GENERAL, test_name)
}

fn role_config(role: &str, test_name: &str) -> (RedisConfig, Configuration) {
    let snapshot = Configuration::load(role, test_name);
    let config = snapshot
        .app
        .redis
        .clone()
        .expect("显式配置缺少 redis 字段；请配置 Required Redis");
    assert_eq!(
        config.mode,
        RedisMode::Required,
        "真实 Redis 测试要求 redis.mode=required"
    );
    (config, snapshot)
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

fn integration_enabled(role: &str) -> bool {
    configuration::enabled(role)
}

fn skip_message() {
    eprintln!("Redis 显式角色配置目录不存在：跳过且真实协议未验收");
}

fn redis_error(error: redis::RedisError) -> String {
    error.to_string()
}

fn identity(error: String) -> String {
    error
}
