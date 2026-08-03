mod common;

use std::sync::Arc;

use ryframe_db::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository,
    entities::{cache_namespace_version, tenant},
};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, TransactionTrait};
use tokio::sync::Barrier;

async fn seed_namespace(db: &DatabaseConnection) {
    let now = chrono::Utc::now();
    tenant::ActiveModel {
        id: ActiveValue::Set(1),
        tenant_id: ActiveValue::Set("system".into()),
        name: ActiveValue::Set("系统租户".into()),
        domain: ActiveValue::Set(None),
        status: ActiveValue::Set(tenant::Model::STATUS_NORMAL.into()),
        expire_at: ActiveValue::Set(None),
        max_users: ActiveValue::Set(100),
        max_roles: ActiveValue::Set(20),
        max_storage_mb: ActiveValue::Set(1024),
        max_requests_per_min: ActiveValue::Set(1000),
        session_version: ActiveValue::Set(1),
        authorization_epoch: ActiveValue::Set(1),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    cache_namespace_version::ActiveModel {
        tenant_id: ActiveValue::Set("system".into()),
        namespace: ActiveValue::Set(CONFIG_CACHE_NAMESPACE.into()),
        version: ActiveValue::Set(0),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// 并发事务必须在同一行锁上串行推进，不能生成重复或跳跃版本。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_namespace_increments_are_contiguous_and_monotonic() {
    const WRITERS: usize = 8;

    let database = common::setup_test_db().await;
    seed_namespace(database.connection()).await;
    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut tasks = Vec::with_capacity(WRITERS);

    for _ in 0..WRITERS {
        let connection = database.connection().clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let transaction = connection.begin().await.unwrap();
            barrier.wait().await;
            let version = CacheNamespaceVersionRepository
                .increment_in_transaction(&transaction, "system", CONFIG_CACHE_NAMESPACE)
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            version
        }));
    }

    let mut versions = Vec::with_capacity(WRITERS);
    for task in tasks {
        versions.push(task.await.unwrap());
    }
    versions.sort_unstable();
    assert_eq!(versions, (1..=WRITERS as i64).collect::<Vec<_>>());
    assert_eq!(
        CacheNamespaceVersionRepository
            .find_version(database.connection(), "system", CONFIG_CACHE_NAMESPACE)
            .await
            .unwrap(),
        WRITERS as i64
    );
}
