use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

/// event_bus 模块测试
/// 从 crates/ryframe-core/src/event_bus.rs 内联测试迁移
use ryframe_core::event_bus::{Event, EventBus, EventHandler, EventResult};

#[derive(Debug)]
struct TestEvent {
    value: i32,
}
impl Event for TestEvent {}

#[derive(Debug)]
struct AnotherEvent {
    msg: String,
}
impl Event for AnotherEvent {}

#[tokio::test]
async fn test_publish_single_handler() {
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI32::new(0));
    let c = counter.clone();

    bus.subscribe_fn(move |event: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        }
    });

    bus.publish(TestEvent { value: 42 }).await;

    // 等待异步处理完成
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

#[tokio::test]
async fn test_publish_multiple_handlers() {
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI32::new(0));

    for _ in 0..3 {
        let c = counter.clone();
        bus.subscribe_fn(move |event: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(event.value, Ordering::SeqCst);
                Ok(())
            }
        });
    }

    bus.publish(TestEvent { value: 10 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 3 个处理器各加 10
    assert_eq!(counter.load(Ordering::SeqCst), 30);
}

#[tokio::test]
async fn test_different_event_types_isolated() {
    let bus = EventBus::new();
    let counter_a = Arc::new(AtomicI32::new(0));
    let counter_b = Arc::new(AtomicI32::new(0));

    let ca = counter_a.clone();
    bus.subscribe_fn(move |e: Arc<TestEvent>| {
        let ca = ca.clone();
        async move {
            ca.fetch_add(e.value, Ordering::SeqCst);
            Ok(())
        }
    });

    let cb = counter_b.clone();
    bus.subscribe_fn(move |e: Arc<AnotherEvent>| {
        let cb = cb.clone();
        async move {
            cb.fetch_add(e.msg.len() as i32, Ordering::SeqCst);
            Ok(())
        }
    });

    bus.publish(TestEvent { value: 5 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(counter_a.load(Ordering::SeqCst), 5);
    assert_eq!(counter_b.load(Ordering::SeqCst), 0); // AnotherEvent 未被发布
}

#[tokio::test]
async fn test_handler_error_isolated() {
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI32::new(0));

    // 第一个处理器会失败
    bus.subscribe_fn(|_: Arc<TestEvent>| async move { Err("模拟失败".into()) });

    // 第二个处理器正常
    let c = counter.clone();
    bus.subscribe_fn(move |e: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(e.value, Ordering::SeqCst);
            Ok(())
        }
    });

    bus.publish(TestEvent { value: 7 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 第二个处理器应该正常执行
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn test_publish_sync() {
    let bus = EventBus::new();
    let counter = Arc::new(AtomicI32::new(0));

    for _ in 0..2 {
        let c = counter.clone();
        bus.subscribe_fn(move |e: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(e.value, Ordering::SeqCst);
                Ok(())
            }
        });
    }

    bus.publish_sync(TestEvent { value: 3 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn test_subscriber_count() {
    let bus = EventBus::new();

    assert_eq!(bus.subscriber_count::<TestEvent>(), 0);

    bus.subscribe_fn(|_: Arc<TestEvent>| async { Ok(()) });
    assert_eq!(bus.subscriber_count::<TestEvent>(), 1);

    bus.subscribe_fn(|_: Arc<TestEvent>| async { Ok(()) });
    assert_eq!(bus.subscriber_count::<TestEvent>(), 2);

    assert_eq!(bus.subscriber_count::<AnotherEvent>(), 0);
}

#[tokio::test]
async fn test_clear() {
    let bus = EventBus::new();
    bus.subscribe_fn(|_: Arc<TestEvent>| async { Ok(()) });
    assert_eq!(bus.subscriber_count::<TestEvent>(), 1);

    bus.clear();
    assert_eq!(bus.subscriber_count::<TestEvent>(), 0);
}

#[tokio::test]
async fn test_publish_no_subscribers() {
    let bus = EventBus::new();
    // 没有订阅者时发布不 panic
    bus.publish(TestEvent { value: 1 }).await;
}

#[tokio::test]
async fn test_trait_event_handler() {
    struct MyHandler(Arc<AtomicI32>);

    #[async_trait::async_trait]
    impl EventHandler<TestEvent> for MyHandler {
        async fn handle(&self, event: Arc<TestEvent>) -> EventResult {
            self.0.fetch_add(event.value, Ordering::SeqCst);
            Ok(())
        }
    }

    let bus = EventBus::new();
    let counter = Arc::new(AtomicI32::new(0));
    bus.subscribe::<TestEvent, _>(MyHandler(counter.clone()));

    bus.publish(TestEvent { value: 99 }).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 99);
}
