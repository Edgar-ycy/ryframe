//! ryframe-core 性能基准测试
//!
//! 测量核心模块的吞吐量和延迟基线：
//! - 事件总线发布/订阅
//! - Snowflake ID 生成
//! - 分页查询构造
//! - 缓存读写与防护

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use criterion::{Criterion, criterion_group, criterion_main};
use ryframe_core::{
    cache::{BreakdownGuard, Cache, CacheStrategy, LocalMemoryCache, NoopCache},
    event_bus::{Event, EventBus},
    repository::PageQuery,
};

// ============ 事件总线 ============

#[derive(Debug)]
struct BenchEvent {
    value: i64,
}
impl Event for BenchEvent {}

fn bench_event_bus_publish_no_subscribers(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();

    c.bench_function("event_bus_publish_no_sub", |b| {
        b.iter(|| {
            rt.block_on(async {
                bus.publish(BenchEvent { value: 42 }).await;
            });
        });
    });
}

fn bench_event_bus_publish_1_subscriber(criterion: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI64::new(0));
    let cnt = counter.clone();

    bus.subscribe_fn(move |e: Arc<BenchEvent>| {
        let cnt = cnt.clone();
        async move {
            cnt.fetch_add(e.value, Ordering::SeqCst);
            Ok(())
        }
    });

    criterion.bench_function("event_bus_publish_1_sub", |b| {
        b.iter(|| {
            rt.block_on(async {
                bus.publish(BenchEvent { value: 1 }).await;
            });
        });
    });
}

fn bench_event_bus_publish_10_subscribers(criterion: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI64::new(0));

    for _ in 0..10 {
        let cnt = counter.clone();
        bus.subscribe_fn(move |e: Arc<BenchEvent>| {
            let cnt = cnt.clone();
            async move {
                cnt.fetch_add(e.value, Ordering::SeqCst);
                Ok(())
            }
        });
    }

    criterion.bench_function("event_bus_publish_10_sub", |b| {
        b.iter(|| {
            rt.block_on(async {
                bus.publish(BenchEvent { value: 1 }).await;
            });
        });
    });
}

fn bench_event_bus_subscribe_fn(c: &mut Criterion) {
    c.bench_function("event_bus_subscribe_fn", |b| {
        b.iter(|| {
            let bus = EventBus::new();
            bus.subscribe_fn(|_: Arc<BenchEvent>| async { Ok(()) });
            std::hint::black_box(bus);
        });
    });
}

// ============ Snowflake ID ============

fn bench_snowflake_next_id(c: &mut Criterion) {
    c.bench_function("snowflake_next_id", |b| {
        b.iter(|| {
            let id = ryframe_common::utils::snowflake::next_snowflake_id();
            std::hint::black_box(id);
        });
    });
}

fn bench_snowflake_batch_1000(c: &mut Criterion) {
    c.bench_function("snowflake_batch_1000", |b| {
        b.iter(|| {
            let mut ids = Vec::with_capacity(1000);
            for _ in 0..1000 {
                ids.push(ryframe_common::utils::snowflake::next_snowflake_id());
            }
            std::hint::black_box(ids);
        });
    });
}

// ============ 分页查询 ============

fn bench_page_query_construct(c: &mut Criterion) {
    c.bench_function("page_query_construct", |b| {
        b.iter(|| {
            let q = PageQuery {
                page: std::hint::black_box(1),
                page_size: std::hint::black_box(10),
            };
            std::hint::black_box(q);
        });
    });
}

fn bench_page_query_offset(c: &mut Criterion) {
    c.bench_function("page_query_offset", |b| {
        b.iter(|| {
            let q = PageQuery {
                page: std::hint::black_box(5),
                page_size: std::hint::black_box(20),
            };
            let offset = q.offset();
            std::hint::black_box(offset);
        });
    });
}

// ============ 缓存性能 ============

fn bench_cache_set_get(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cache = LocalMemoryCache::unlimited();

    c.bench_function("cache_local_set", |b| {
        b.iter(|| {
            rt.block_on(async {
                cache.set("bench_key", &"bench_value", 60).await.unwrap();
            });
        });
    });

    // 预先写入一个 key
    rt.block_on(async {
        cache.set("bench_get", &"cached_value", 60).await.unwrap();
    });

    c.bench_function("cache_local_get_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let v: Option<String> = cache.get("bench_get").await.unwrap();
                std::hint::black_box(v);
            });
        });
    });

    c.bench_function("cache_local_get_miss", |b| {
        b.iter(|| {
            rt.block_on(async {
                let v: Option<String> = cache.get("nonexistent").await.unwrap();
                std::hint::black_box(v);
            });
        });
    });
}

fn bench_cache_breakdown_guard(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cache = LocalMemoryCache::unlimited();
    let guard = BreakdownGuard::new(cache);

    c.bench_function("breakdown_guard_hit", |b| {
        // 预填充
        rt.block_on(async {
            guard.inner().set("hot_key", &"loaded", 3600).await.unwrap();
        });

        b.iter(|| {
            rt.block_on(async {
                let v = guard
                    .get_or_load_guarded::<String, _, _>("hot_key", 3600, || async {
                        Ok(Some("miss".to_string()))
                    })
                    .await
                    .unwrap();
                std::hint::black_box(v);
            });
        });
    });
}

fn bench_cache_get_or_load(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cache = CacheStrategy::new(LocalMemoryCache::unlimited())
        .with_avalanche_jitter(0.0)
        .with_null_cache_ttl(60);

    // 预填充
    rt.block_on(async {
        cache.set("load_key", &"preloaded", 3600).await.unwrap();
    });

    c.bench_function("cache_get_or_load_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let v: String = cache
                    .get_or_load("load_key", 3600, || async {
                        Ok("never_called".to_string())
                    })
                    .await
                    .unwrap();
                std::hint::black_box(v);
            });
        });
    });
}

fn bench_noop_cache_overhead(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let cache = NoopCache;

    c.bench_function("noop_cache_set_get", |b| {
        b.iter(|| {
            rt.block_on(async {
                cache.set("k", &"v", 60).await.unwrap();
                let v: Option<String> = cache.get("k").await.unwrap();
                std::hint::black_box(v);
            });
        });
    });
}

criterion_group!(
    benches,
    bench_event_bus_publish_no_subscribers,
    bench_event_bus_publish_1_subscriber,
    bench_event_bus_publish_10_subscribers,
    bench_event_bus_subscribe_fn,
    bench_snowflake_next_id,
    bench_snowflake_batch_1000,
    bench_page_query_construct,
    bench_page_query_offset,
    bench_cache_set_get,
    bench_cache_breakdown_guard,
    bench_cache_get_or_load,
    bench_noop_cache_overhead,
);
criterion_main!(benches);
