use std::time::Duration;

/// distributed_lock 模块测试
/// 从 crates/ryframe-core/src/distributed_lock.rs 内联测试迁移
use ryframe_core::distributed_lock::{DistributedLock, LocalDistributedLock, RedisDistributedLock};

#[test]
fn test_holder_id_unique() {
    let id1 = RedisDistributedLock::holder_id();
    let id2 = RedisDistributedLock::holder_id();
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn local_lock_preserves_mutual_exclusion_and_releases_on_drop() {
    let lock = LocalDistributedLock::new();
    let guard = lock
        .try_acquire("test", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert!(
        lock.try_acquire("test", Duration::from_secs(10))
            .await
            .unwrap()
            .is_none()
    );
    drop(guard);
    assert!(
        lock.try_acquire("test", Duration::from_secs(10))
            .await
            .unwrap()
            .is_some()
    );
}
