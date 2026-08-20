use std::{future::Future, pin::Pin, sync::Arc};

use ryframe_adapters::{
    RedisClient,
    idempotency::{RedisIdempotencyStore, RemoteIdempotencyReservation},
};
use ryframe_api::middleware::idempotency::{
    HttpIdempotencyStore, IdempotencyStoreFuture, StoredIdempotencyReservation,
};

struct RedisIdempotencyStoreBridge {
    store: RedisIdempotencyStore,
}

impl HttpIdempotencyStore for RedisIdempotencyStoreBridge {
    fn reserve<'a>(
        &'a self,
        key: &'a str,
        fingerprint: &'a str,
        processing_ttl_secs: u64,
    ) -> IdempotencyStoreFuture<'a, StoredIdempotencyReservation> {
        Box::pin(async move {
            self.store
                .reserve(key, fingerprint, processing_ttl_secs)
                .await
                .map(|reservation| match reservation {
                    RemoteIdempotencyReservation::Acquired => {
                        StoredIdempotencyReservation::Acquired
                    }
                    RemoteIdempotencyReservation::Processing => {
                        StoredIdempotencyReservation::Processing
                    }
                    RemoteIdempotencyReservation::Conflict => {
                        StoredIdempotencyReservation::Conflict
                    }
                    RemoteIdempotencyReservation::Completed(response) => {
                        StoredIdempotencyReservation::Completed(response)
                    }
                    RemoteIdempotencyReservation::NonReplayable => {
                        StoredIdempotencyReservation::NonReplayable
                    }
                })
        })
    }

    fn begin_execution<'a>(
        &'a self,
        key: &'a str,
        fingerprint: &'a str,
        completed_ttl_secs: u64,
    ) -> IdempotencyStoreFuture<'a, ()> {
        Box::pin(async move {
            self.store
                .begin_execution(key, fingerprint, completed_ttl_secs)
                .await
        })
    }

    fn complete<'a>(
        &'a self,
        key: &'a str,
        fingerprint: &'a str,
        response: &'a str,
        completed_ttl_secs: u64,
    ) -> IdempotencyStoreFuture<'a, ()> {
        Box::pin(async move {
            self.store
                .complete(key, fingerprint, response, completed_ttl_secs)
                .await
        })
    }

    fn mark_non_replayable<'a>(
        &'a self,
        key: &'a str,
        fingerprint: &'a str,
        completed_ttl_secs: u64,
    ) -> IdempotencyStoreFuture<'a, ()> {
        Box::pin(async move {
            self.store
                .mark_non_replayable(key, fingerprint, completed_ttl_secs)
                .await
        })
    }

    fn release<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move { self.store.release(key).await })
    }
}

pub fn store(redis: Option<RedisClient>) -> Option<Arc<dyn HttpIdempotencyStore>> {
    redis.map(|redis| {
        Arc::new(RedisIdempotencyStoreBridge {
            store: RedisIdempotencyStore::new(redis),
        }) as Arc<dyn HttpIdempotencyStore>
    })
}
