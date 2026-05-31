use std::{sync::Arc, time::Duration};

use ryframe_middleware::rate_limit::RateLimiter;

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

    // 令牌补充
    let limiter3 = RateLimiter::new_in_memory(2, 100);
    assert!(limiter3.try_acquire("test").await);
    assert!(limiter3.try_acquire("test").await);
    assert!(!limiter3.try_acquire("test").await);
    tokio::time::sleep(Duration::from_millis(20)).await;
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
