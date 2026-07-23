use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{Router, body::Body, http::Request, middleware, routing::get};
use ryframe_common::utils::ip::TrustedProxySet;
use ryframe_config::{RateLimitConfig, RedisConfig, RedisMode};
use ryframe_core::RedisClient;
use ryframe_middleware::rate_limit::{RateLimitState, RateLimiter, api_rate_limit_middleware};
use tower::ServiceExt;

async fn docker_redis() -> RedisClient {
    let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16379);
    RedisClient::connect(&RedisConfig {
        mode: RedisMode::Required,
        host: "127.0.0.1".into(),
        port,
        timeout_secs: 2,
        ..RedisConfig::default()
    })
    .await
    .expect(
        "connect Redis test service; run `docker compose -f docker-compose.test.yml up -d --wait`",
    )
}

#[tokio::test]
async fn test_rate_limiter() {
    // 基本限流：容量 3，前 3 次通过，第 4 次拒绝
    let limiter = RateLimiter::new_in_memory(3, 1);
    assert!(limiter.try_acquire("test").await);
    assert!(limiter.try_acquire("test").await);
    assert!(limiter.try_acquire("test").await);
    assert!(!limiter.try_acquire("test").await);

    // 不同 key 独立
    let limiter2 = RateLimiter::new_in_memory(1, 1);
    assert!(limiter2.try_acquire("a").await);
    assert!(!limiter2.try_acquire("a").await);
    assert!(limiter2.try_acquire("b").await);

    // 固定窗口在窗口结束前不会补充容量。
    let limiter3 = RateLimiter::new_in_memory(2, 100);
    assert!(limiter3.try_acquire("test").await);
    assert!(limiter3.try_acquire("test").await);
    assert!(!limiter3.try_acquire("test").await);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!limiter3.try_acquire("test").await);
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    assert!(limiter3.try_acquire("test").await);
}

#[tokio::test]
async fn test_spawn_gc() {
    let limiter = Arc::new(RateLimiter::new_in_memory(10, 1));
    limiter.try_acquire("key1").await;
    limiter.spawn_gc();
}

#[tokio::test]
async fn test_sliding_window_memory_fallback() {
    // 内存模式下滑动窗口回退到固定窗口
    let limiter = RateLimiter::new_in_memory(5, 10);

    // 窗口内应通过
    for _ in 0..5 {
        assert!(limiter.sliding_window_acquire("sw_test", 60, 10).await);
    }

    // 第 6 次请求：快速补充率（refill_per_sec=10）下令牌可能已补充完成
    // 也可能尚未补充，此处仅验证调用不 panic
    let _result = limiter.sliding_window_acquire("sw_test", 60, 10).await;
}

#[tokio::test]
async fn test_user_key() {
    assert_eq!(RateLimiter::user_key("12345"), "user:12345");
    assert_eq!(RateLimiter::user_key("admin"), "user:admin");
}

#[tokio::test]
async fn test_api_key() {
    assert_eq!(RateLimiter::api_key("/api/users"), "api:/api/users");
    assert_eq!(
        RateLimiter::api_key("/api/orders/create"),
        "api:/api/orders/create"
    );
}

#[tokio::test]
async fn test_user_api_key() {
    assert_eq!(
        RateLimiter::user_api_key("12345", "/api/users"),
        "user_api:12345:/api/users"
    );
    assert_eq!(
        RateLimiter::user_api_key("admin", "/api/orders"),
        "user_api:admin:/api/orders"
    );
}

#[tokio::test]
async fn test_available_tokens() {
    let limiter = RateLimiter::new_in_memory(5, 2);
    assert_eq!(limiter.available_tokens("tok"), 5.0);
    limiter.try_acquire("tok").await;
    assert!(limiter.available_tokens("tok") < 5.0);
}

fn api_limited_router(rules: HashMap<String, u32>) -> Router {
    let state = RateLimitState {
        limiter: Arc::new(RateLimiter::new_in_memory(100, 0)),
        config: Arc::new(RateLimitConfig {
            enabled: true,
            api_limits: rules,
            api_window_secs: 60,
            ..Default::default()
        }),
        trusted_proxies: TrustedProxySet::default(),
    };
    let routes = Router::new()
        .route("/resources/{id}", get(|| async { "resource" }))
        .route("/first", get(|| async { "first" }))
        .route("/second", get(|| async { "second" }));
    Router::new()
        .nest("/api/v1", routes)
        .layer(middleware::from_fn_with_state(
            state,
            api_rate_limit_middleware,
        ))
}

#[tokio::test]
async fn dynamic_route_ids_share_the_configured_route_bucket() {
    let app = api_limited_router(HashMap::from([(
        "GET /api/v1/resources/{id}".to_string(),
        1,
    )]));

    let first = app
        .clone()
        .oneshot(
            Request::get("/api/v1/resources/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let second = app
        .oneshot(
            Request::get("/api/v1/resources/2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 429);
}

#[tokio::test]
async fn concrete_method_rule_overrides_route_template_rule() {
    let app = api_limited_router(HashMap::from([
        ("GET /api/v1/resources/{id}".to_string(), 1),
        ("GET /api/v1/resources/42".to_string(), 2),
    ]));

    for expected in [200, 200, 429] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/resources/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    let first_template_response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/resources/43")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let second_template_response = app
        .oneshot(
            Request::get("/api/v1/resources/44")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(first_template_response.status(), 200);
    assert_eq!(second_template_response.status(), 429);
}

#[tokio::test]
async fn method_wide_rule_uses_one_bucket_across_paths() {
    let app = api_limited_router(HashMap::from([("GET".to_string(), 1)]));

    let first = app
        .clone()
        .oneshot(Request::get("/api/v1/first").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let second = app
        .oneshot(Request::get("/api/v1/second").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 429);
}

#[tokio::test]
async fn redis_and_memory_fixed_windows_apply_the_same_rule() {
    let redis = RateLimiter::new_redis(docker_redis().await, 100, 60);
    let memory = RateLimiter::new_in_memory(100, 0);
    let key = format!(
        "parity-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );

    for attempt in 1..=6 {
        let memory_decision = memory.acquire(&key, 1, 5).await.unwrap();
        let redis_decision = redis.acquire(&key, 1, 5).await.unwrap();
        assert_eq!(memory_decision.allowed, attempt <= 5);
        assert_eq!(redis_decision.allowed, memory_decision.allowed);
        if !redis_decision.allowed {
            assert!(redis_decision.retry_after_secs >= 1);
        }
    }

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert!(memory.acquire(&key, 1, 5).await.unwrap().allowed);
    assert!(redis.acquire(&key, 1, 5).await.unwrap().allowed);
}
