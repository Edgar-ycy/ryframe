use std::sync::Arc;

use crate::{ControlDatabaseCluster, OutboxEventRepository, RecordOutboxEvent};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use ryframe_application::{
    AUDIT_AGGREGATE_TYPE, OUTBOX_MAX_ATTEMPTS, bind_current_audit, validate_audit_event,
};
use ryframe_application::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutboxPersistencePort,
    AuditTransactionBinding, PersistenceFuture,
};

pub fn outbox(database: ControlDatabaseCluster) -> Arc<dyn AuditOutboxPersistencePort> {
    Arc::new(DatabaseAuditOutboxPersistence { database })
}

struct DatabaseAuditOutboxPersistence {
    database: ControlDatabaseCluster,
}

impl AuditOutboxPersistencePort for DatabaseAuditOutboxPersistence {
    fn record<'a>(
        &'a self,
        event: &'a AuditOperationEvent,
        max_attempts: i32,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            let result = record_event_in_transaction(&transaction, event, max_attempts).await;
            match result {
                Ok(()) => transaction.commit().await.map_err(database_error),
                Err(error) => {
                    let _ = transaction.rollback().await;
                    Err(error)
                }
            }
        })
    }
}

/// 把当前请求的成功审计事件写入调用方业务事务。
///
/// 非 HTTP 调用链返回 `None`。HTTP 写请求返回绑定句柄，调用方需在事务提交成功后标记。
pub async fn record_current_audit_in_transaction(
    transaction: &DatabaseTransaction,
) -> AppResult<Option<AuditTransactionBinding>> {
    let Some((event, binding)) = bind_current_audit() else {
        return Ok(None);
    };
    record_event_in_transaction(transaction, &event, OUTBOX_MAX_ATTEMPTS).await?;
    Ok(Some(binding))
}

/// 在提交调用方事务前自动写入当前请求审计事件，并在提交成功后完成绑定标记。
pub async fn commit_current_audit(transaction: DatabaseTransaction) -> AppResult<()> {
    let binding = record_current_audit_in_transaction(&transaction).await?;
    transaction.commit().await.map_err(database_error)?;
    if let Some(binding) = binding {
        binding.mark_committed();
    }
    Ok(())
}

async fn record_event_in_transaction(
    transaction: &DatabaseTransaction,
    event: &AuditOperationEvent,
    max_attempts: i32,
) -> AppResult<()> {
    validate_audit_event(event)?;
    let now = crate::repositories::database_utc_now(transaction).await?;
    let payload = serde_json::to_value(event)
        .map_err(|error| AppError::Internal(format!("审计事件序列化失败: {error}")))?;
    let trace_context = ryframe_application::current_trace_context();
    OutboxEventRepository
        .record_in_transaction(
            transaction,
            RecordOutboxEvent {
                tenant_id: Some(event.tenant_id.clone()),
                event_type: AUDIT_OPERATION_OUTBOX_EVENT_TYPE.to_owned(),
                aggregate_type: AUDIT_AGGREGATE_TYPE.to_owned(),
                aggregate_id: event.event_id.clone(),
                payload,
                available_at: now,
                max_attempts,
                dedupe_key: Some(event.event_id.clone()),
                traceparent: trace_context.traceparent,
                tracestate: trace_context.tracestate,
            },
            now,
        )
        .await?;
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
