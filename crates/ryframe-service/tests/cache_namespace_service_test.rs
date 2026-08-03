mod common;

use std::sync::Arc;

use async_trait::async_trait;
use ryframe_db::{CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, entities::outbox_event};
use ryframe_service::{
    AuthorizationCache, AuthorizationCacheBackend, AuthorizationCacheLookup, AuthorizationSnapshot,
    NamespaceCacheLookup, TenantCacheLookup,
};
use sea_orm::{EntityTrait, TransactionTrait};
use tokio::sync::Barrier;

struct NoopCacheBackend;

#[async_trait]
impl AuthorizationCacheBackend for NoopCacheBackend {
    async fn lookup_snapshot(
        &self,
        _tenant_id: &str,
        _user_id: i64,
    ) -> Result<AuthorizationCacheLookup, String> {
        Err("本测试不读取授权快照".into())
    }

    async fn store_snapshot(&self, _snapshot: &AuthorizationSnapshot) -> Result<bool, String> {
        Ok(false)
    }

    async fn update_tenant_epoch(
        &self,
        _tenant_id: &str,
        _authorization_epoch: i32,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn update_user_version(
        &self,
        _tenant_id: &str,
        _user_id: i64,
        _authorization_version: i32,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn read_tenant_value(
        &self,
        _tenant_id: &str,
        _namespace: &str,
    ) -> Result<Option<TenantCacheLookup>, String> {
        Ok(None)
    }

    async fn store_tenant_value(
        &self,
        _tenant_id: &str,
        _namespace: &str,
        _authorization_epoch: i32,
        _value: &str,
        _ttl_secs: u64,
    ) -> Result<bool, String> {
        Ok(false)
    }

    async fn update_namespace_version(
        &self,
        _tenant_id: &str,
        _namespace: &str,
        _namespace_version: i64,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn read_namespace_value(
        &self,
        _tenant_id: &str,
        _namespace: &str,
        _item: &str,
    ) -> Result<Option<NamespaceCacheLookup>, String> {
        Ok(None)
    }

    async fn store_namespace_value(
        &self,
        _tenant_id: &str,
        _namespace: &str,
        _item: &str,
        _namespace_version: i64,
        _value: &str,
        _ttl_secs: u64,
    ) -> Result<bool, String> {
        Ok(false)
    }
}

/// 业务版本与对应 Outbox 事件必须在并发事务中保持一一对应。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_version_advances_commit_matching_outbox_events() {
    const WRITERS: usize = 8;

    let database = common::setup_test_db().await;
    let cache = AuthorizationCache::from_backend(Arc::new(NoopCacheBackend), false);
    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut tasks = Vec::with_capacity(WRITERS);

    for _ in 0..WRITERS {
        let connection = database.connection().clone();
        let cache = cache.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let transaction = connection.begin().await.unwrap();
            barrier.wait().await;
            let version = cache
                .record_namespace_version_in_transaction(
                    &transaction,
                    "system",
                    CONFIG_CACHE_NAMESPACE,
                )
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            version
        }));
    }

    let mut committed_versions = Vec::with_capacity(WRITERS);
    for task in tasks {
        committed_versions.push(task.await.unwrap());
    }
    committed_versions.sort_unstable();
    assert_eq!(committed_versions, (1..=WRITERS as i64).collect::<Vec<_>>());

    let mut event_versions = outbox_event::Entity::find()
        .all(database.connection())
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload["namespace_version"].as_i64().unwrap())
        .collect::<Vec<_>>();
    event_versions.sort_unstable();
    assert_eq!(event_versions, committed_versions);
    assert_eq!(
        CacheNamespaceVersionRepository
            .find_version(database.connection(), "system", CONFIG_CACHE_NAMESPACE)
            .await
            .unwrap(),
        WRITERS as i64
    );
}
