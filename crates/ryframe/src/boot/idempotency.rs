use std::sync::Arc;

use ryframe_adapters::{
    RedisClient,
    idempotency::{RedisIdempotencyStore, RemoteIdempotencyReservation},
};
use ryframe_api::middleware::idempotency::{HttpIdempotencyStore, StoredIdempotencyReservation};

struct RedisIdempotencyStoreBridge {
    store: RedisIdempotencyStore,
}

#[async_trait::async_trait]
impl HttpIdempotencyStore for RedisIdempotencyStoreBridge {
    async fn reserve(
        &self,
        key: &str,
        fingerprint: &str,
        processing_ttl_secs: u64,
    ) -> Result<StoredIdempotencyReservation, String> {
        self.store
            .reserve(key, fingerprint, processing_ttl_secs)
            .await
            .map(|reservation| match reservation {
                RemoteIdempotencyReservation::Acquired => StoredIdempotencyReservation::Acquired,
                RemoteIdempotencyReservation::Processing => {
                    StoredIdempotencyReservation::Processing
                }
                RemoteIdempotencyReservation::Conflict => StoredIdempotencyReservation::Conflict,
                RemoteIdempotencyReservation::Completed(response) => {
                    StoredIdempotencyReservation::Completed(response)
                }
                RemoteIdempotencyReservation::NonReplayable => {
                    StoredIdempotencyReservation::NonReplayable
                }
            })
    }

    async fn begin_execution(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        self.store
            .begin_execution(key, fingerprint, completed_ttl_secs)
            .await
    }

    async fn complete(
        &self,
        key: &str,
        fingerprint: &str,
        response: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        self.store
            .complete(key, fingerprint, response, completed_ttl_secs)
            .await
    }

    async fn mark_non_replayable(
        &self,
        key: &str,
        fingerprint: &str,
        completed_ttl_secs: u64,
    ) -> Result<(), String> {
        self.store
            .mark_non_replayable(key, fingerprint, completed_ttl_secs)
            .await
    }

    async fn release(&self, key: &str) {
        self.store.release(key).await
    }
}

pub fn store(redis: Option<RedisClient>) -> Option<Arc<dyn HttpIdempotencyStore>> {
    redis.map(|redis| {
        Arc::new(RedisIdempotencyStoreBridge {
            store: RedisIdempotencyStore::new(redis),
        }) as Arc<dyn HttpIdempotencyStore>
    })
}
