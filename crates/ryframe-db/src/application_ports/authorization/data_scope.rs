use crate::{
    OutboxEventRepository, RecordOutboxEvent, TenantRepository,
    application_ports::transaction::DatabasePortTransaction,
};
use ryframe_application::{
    DATA_SCOPE_PROJECTION_OUTBOX_EVENT_TYPE, DataScopeMutationTransaction, DataScopeProjectionEvent,
};

#[async_trait::async_trait]
impl DataScopeMutationTransaction for DatabasePortTransaction {
    async fn record_data_scope_change(&self, tenant_id: &str) -> ryframe_kernel::AppResult<i64> {
        let source_version = TenantRepository
            .increment_data_scope_version_in_txn(self, tenant_id)
            .await?;
        let now = crate::repositories::database_utc_now_time(self).await?;
        let payload = serde_json::to_value(DataScopeProjectionEvent {
            tenant_id: tenant_id.to_owned(),
            source_version,
        })
        .map_err(|error| {
            ryframe_kernel::AppError::Internal(format!("数据范围投影事件序列化失败: {error}"))
        })?;
        OutboxEventRepository
            .record_in_transaction(
                self,
                RecordOutboxEvent {
                    tenant_id: Some(tenant_id.to_owned()),
                    event_type: DATA_SCOPE_PROJECTION_OUTBOX_EVENT_TYPE.to_owned(),
                    aggregate_type: "tenant_data_scope".to_owned(),
                    aggregate_id: tenant_id.to_owned(),
                    payload,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("{tenant_id}:{source_version}")),
                    traceparent: None,
                    tracestate: None,
                },
                now,
            )
            .await
            .map(|_| source_version)
    }
}
