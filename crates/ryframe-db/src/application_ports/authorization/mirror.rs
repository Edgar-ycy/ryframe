use super::super::control_transaction::DatabasePortTransaction;
use crate::{
    CacheNamespaceVersionRepository, OutboxEventRepository, RecordOutboxEvent, TenantRepository,
    UserRepository,
};

use ryframe_application::{
    AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE, PersistenceFuture,
    ports::authorization::{AuthorizationMirrorEvent, AuthorizationMirrorTransaction},
};

impl AuthorizationMirrorTransaction for DatabasePortTransaction {
    fn increment_user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            UserRepository
                .increment_authorization_versions(self, tenant_id, user_ids)
                .await
        })
    }

    fn user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<(i64, i32)>> {
        Box::pin(async move {
            UserRepository
                .find_authorization_versions(self, tenant_id, user_ids)
                .await
        })
    }

    fn increment_tenant_epoch<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, i32> {
        Box::pin(async move {
            TenantRepository
                .increment_authorization_epoch_in_txn(self, tenant_id)
                .await
        })
    }

    fn increment_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            CacheNamespaceVersionRepository
                .increment_in_transaction(self, tenant_id, namespace)
                .await
        })
    }

    fn database_now(&self) -> PersistenceFuture<'_, chrono::DateTime<chrono::Utc>> {
        Box::pin(async move { OutboxEventRepository.database_utc_now(self).await })
    }

    fn record(&self, event: AuthorizationMirrorEvent) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            let available_at = event.available_at;
            OutboxEventRepository
                .record_in_transaction(
                    self,
                    RecordOutboxEvent {
                        tenant_id: Some(event.tenant_id),
                        event_type: AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE.to_owned(),
                        aggregate_type: event.aggregate_type,
                        aggregate_id: event.aggregate_id,
                        payload: event.payload,
                        available_at,
                        max_attempts: event.max_attempts,
                        dedupe_key: Some(event.dedupe_key),
                        traceparent: event.traceparent,
                        tracestate: event.tracestate,
                    },
                    available_at,
                )
                .await
                .map(|_| ())
        })
    }
}
