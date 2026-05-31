//! ryframe-middleware 性能基准测试
//!
//! 测量中间件核心操作的吞吐量和延迟：
//! - 令牌桶限流（内存模式）
//! - 安全响应头构建
//! - XSS 过滤（JSON 净化）
//! - 请求 ID 生成
//! - 限流 key 生成

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use ryframe_middleware::rate_limit::RateLimiter;

// ============ 令牌桶限流（内存模式） ============

fn bench_rate_limit_in_memory_acquire(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let limiter = Arc::new(RateLimiter::new_in_memory(100, 100));

    c.bench_function("rate_limit_in_memory_acquire", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = limiter
                    .try_acquire(std::hint::black_box("bench_ip_192.168.1.1"))
                    .await;
            });
        });
    });
}

fn bench_rate_limit_in_memory_10k_concurrent_keys(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let limiter = Arc::new(RateLimiter::new_in_memory(100, 100));

    // 预热：填充 10000 个不同 key
    rt.block_on(async {
        for i in 0..10000 {
            limiter
                .try_acquire(&format!("ip_192.168.1.{}", i % 256))
                .await;
        }
    });

    c.bench_function("rate_limit_10k_keys_acquire", |b| {
        let mut counter = 0u32;
        b.iter(|| {
            let idx = counter % 10000;
            counter = counter.wrapping_add(1);
            rt.block_on(async {
                let _ = limiter.try_acquire(&format!("ip_192.168.1.{}", idx)).await;
            });
        });
    });
}

fn bench_rate_limit_key_generation(c: &mut Criterion) {
    c.bench_function("rate_limit_user_key", |b| {
        b.iter(|| {
            let _ = RateLimiter::user_key(std::hint::black_box("10001"));
        });
    });

    c.bench_function("rate_limit_api_key", |b| {
        b.iter(|| {
            let _ = RateLimiter::api_key(std::hint::black_box("/api/v1/system/users"));
        });
    });

    c.bench_function("rate_limit_user_api_key", |b| {
        b.iter(|| {
            let _ = RateLimiter::user_api_key(
                std::hint::black_box("10001"),
                std::hint::black_box("/api/v1/system/users"),
            );
        });
    });
}

// ============ 安全响应头构建 ============

fn bench_security_headers_config_default(c: &mut Criterion) {
    c.bench_function("security_headers_config_default", |b| {
        b.iter(|| {
            let cfg = ryframe_middleware::security_headers::SecurityHeadersConfig::default();
            std::hint::black_box(cfg);
        });
    });
}

fn bench_security_headers_config_strict(c: &mut Criterion) {
    c.bench_function("security_headers_config_strict", |b| {
        b.iter(|| {
            let cfg = ryframe_middleware::security_headers::SecurityHeadersConfig::strict();
            std::hint::black_box(cfg);
        });
    });
}

fn bench_security_headers_config_dev(c: &mut Criterion) {
    c.bench_function("security_headers_config_development", |b| {
        b.iter(|| {
            let cfg = ryframe_middleware::security_headers::SecurityHeadersConfig::development();
            std::hint::black_box(cfg);
        });
    });
}

// ============ XSS 过滤 ============

fn bench_xss_filter_simple_json(c: &mut Criterion) {
    let input = br#"{"name": "<script>alert(1)</script>", "age": 25}"#;
    let bytes = axum::body::Bytes::from_static(input);

    c.bench_function("xss_filter_simple_json", |b| {
        b.iter(|| {
            let sanitized =
                ryframe_middleware::xss_filter::sanitize_json_bytes(std::hint::black_box(&bytes));
            std::hint::black_box(sanitized);
        });
    });
}

fn bench_xss_filter_nested_json(c: &mut Criterion) {
    let input = br#"{
        "user": {"name": "<img onerror=alert(1) src=x>", "email": "test@test.com"},
        "items": [
            {"title": "<b>hello</b>", "desc": "<a href=javascript:evil()>click</a>"},
            {"title": "safe", "desc": "normal text"}
        ],
        "meta": {"key1": "<script>bad</script>", "key2": "ok"}
    }"#;
    let bytes = axum::body::Bytes::from_static(input);

    c.bench_function("xss_filter_nested_json", |b| {
        b.iter(|| {
            let sanitized =
                ryframe_middleware::xss_filter::sanitize_json_bytes(std::hint::black_box(&bytes));
            std::hint::black_box(sanitized);
        });
    });
}

fn bench_xss_filter_no_xss(c: &mut Criterion) {
    let input = br#"{"name": "John Doe", "email": "john@example.com", "roles": ["admin", "user"]}"#;
    let bytes = axum::body::Bytes::from_static(input);

    c.bench_function("xss_filter_clean_json", |b| {
        b.iter(|| {
            let sanitized =
                ryframe_middleware::xss_filter::sanitize_json_bytes(std::hint::black_box(&bytes));
            std::hint::black_box(sanitized);
        });
    });
}

// ============ 请求 ID 生成 ============

fn bench_request_id_uuid_v7(c: &mut Criterion) {
    c.bench_function("request_id_uuid_v7", |b| {
        b.iter(|| {
            let id = uuid::Uuid::now_v7().to_string();
            std::hint::black_box(id);
        });
    });
}

criterion_group!(
    benches,
    bench_rate_limit_in_memory_acquire,
    bench_rate_limit_in_memory_10k_concurrent_keys,
    bench_rate_limit_key_generation,
    bench_security_headers_config_default,
    bench_security_headers_config_strict,
    bench_security_headers_config_dev,
    bench_xss_filter_simple_json,
    bench_xss_filter_nested_json,
    bench_xss_filter_no_xss,
    bench_request_id_uuid_v7,
);
criterion_main!(benches);
