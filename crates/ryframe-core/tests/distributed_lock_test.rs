use std::time::Duration;

/// distributed_lock 模块测试
/// 从 crates/ryframe-core/src/distributed_lock.rs 内联测试迁移
use ryframe_core::distributed_lock::{DistributedLock, NoopLock, RedisDistributedLock};

#[test]
fn test_holder_id_unique() {
    let id1 = RedisDistributedLock::holder_id();
    let id2 = RedisDistributedLock::holder_id();
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn test_noop_lock_always_acquires() {
    let lock = NoopLock::new();
    let guard = lock
        .try_acquire("test", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(guard.is_some());
}
