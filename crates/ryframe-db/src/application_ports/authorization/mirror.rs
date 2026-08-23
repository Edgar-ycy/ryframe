use super::super::transaction::DatabasePortTransaction;
use crate::{
    CacheNamespaceVersionRepository, OutboxEventRepository, RecordOutboxEvent, TenantRepository,
    UserRepository,
};

use ryframe_application::{
    AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE,
    ports::authorization::{AuthorizationMirrorEvent, AuthorizationMirrorTransaction},
};

#[async_trait::async_trait]
impl AuthorizationMirrorTransaction for DatabasePortTransaction {
    async fn increment_user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<u64> {
        UserRepository
            .increment_authorization_versions(self, tenant_id, user_ids)
            .await
    }

    async fn user_versions<'a>(
        &'a self,
        tenant_id: &'a str,
        user_ids: &'a [i64],
    ) -> ryframe_kernel::AppResult<Vec<(i64, i32)>> {
        UserRepository
            .find_authorization_versions(self, tenant_id, user_ids)
            .await
    }

    async fn increment_tenant_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<i32> {
        TenantRepository
            .increment_authorization_epoch_in_txn(self, tenant_id)
            .await
    }

    async fn increment_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> ryframe_kernel::AppResult<i64> {
        CacheNamespaceVersionRepository
            .increment_in_transaction(self, tenant_id, namespace)
            .await
    }

    async fn database_now(&self) -> ryframe_kernel::AppResult<chrono::DateTime<chrono::Utc>> {
        crate::repositories::database_utc_now(self).await
    }

    async fn record(&self, event: AuthorizationMirrorEvent) -> ryframe_kernel::AppResult<()> {
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
    }
}
