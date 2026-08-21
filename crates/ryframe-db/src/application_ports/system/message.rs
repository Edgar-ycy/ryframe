use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, MessageAudienceKind as DatabaseAudienceKind,
    MessageAudienceSelector as DatabaseAudience, MessageInboxQuery, MessageRepository,
    OutboxEventRepository, PublishMessageCommand, RecipientMessage, RecordOutboxEvent,
    entities::message,
};
use sea_orm::{EntityTrait, TransactionTrait};

use ryframe_application::{
    ControlTransaction, PersistenceFuture,
    ports::system::{
        MessageAudienceRecordKind, MessageInboxFilter, MessageOutboxRecord, MessagePage,
        MessagePersistencePort, MessageRecipientRecord, MessageRecord, MessageTransaction,
        PublishMessageRecord, PublishedMessageRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn MessagePersistencePort> {
    Arc::new(DatabaseMessagePersistence { database })
}

struct DatabaseMessagePersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseMessageTransaction {
    transaction: sea_orm::DatabaseTransaction,
}

impl MessagePersistencePort for DatabaseMessagePersistence {
    fn inbox<'a>(&'a self, filter: MessageInboxFilter<'a>) -> PersistenceFuture<'a, MessagePage> {
        Box::pin(async move {
            let page = MessageRepository
                .inbox(
                    self.database.write(),
                    MessageInboxQuery {
                        tenant_id: filter.tenant_id,
                        user_id: filter.user_id,
                        cursor: filter.cursor,
                        limit: filter.limit,
                        unread_only: filter.unread_only,
                        unacknowledged_only: filter.unacknowledged_only,
                        now: filter.now,
                    },
                )
                .await?;
            Ok(MessagePage {
                records: page.records.into_iter().map(to_recipient_record).collect(),
                next_cursor: page.next_cursor,
            })
        })
    }

    fn unacknowledged_recipients<'a>(
        &'a self,
        message_id: i64,
        user_ids: Option<&'a [i64]>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, Vec<MessageRecipientRecord>> {
        Box::pin(async move {
            let records = match user_ids {
                Some(user_ids) => {
                    MessageRepository
                        .unacknowledged_recipients_for_message_for_online_users(
                            self.database.write(),
                            message_id,
                            user_ids,
                            now,
                        )
                        .await?
                }
                None => {
                    MessageRepository
                        .unacknowledged_recipients_for_message(
                            self.database.write(),
                            message_id,
                            now,
                        )
                        .await?
                }
            };
            Ok(records.into_iter().map(to_recipient_record).collect())
        })
    }

    fn unread_count<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            MessageRepository
                .unread_count(self.database.write(), tenant_id, user_id, now)
                .await
        })
    }

    fn find_message(&self, message_id: i64) -> PersistenceFuture<'_, Option<MessageRecord>> {
        Box::pin(async move {
            Ok(message::Entity::find_by_id(message_id)
                .one(self.database.write())
                .await
                .map_err(database_error)?
                .map(to_message_record))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn MessageTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseMessageTransaction { transaction }) as Box<dyn MessageTransaction>)
        })
    }
}

impl MessageTransaction for DatabaseMessageTransaction {
    fn publish(
        &self,
        command: PublishMessageRecord,
        max_recipients: u64,
    ) -> PersistenceFuture<'_, PublishedMessageRecord> {
        Box::pin(async move {
            let mut published = MessageRepository
                .publish_in_transaction(
                    &self.transaction,
                    PublishMessageCommand {
                        tenant_id: command.tenant_id,
                        topic: command.topic,
                        title_text: command.title_text,
                        body_text: command.body_text,
                        title_key: command.title_key,
                        body_key: command.body_key,
                        args_json: command.args_json,
                        severity: command.severity,
                        payload_json: command.payload_json,
                        source_type: command.source_type,
                        source_id: command.source_id,
                        created_by: command.created_by,
                        published_at: command.published_at,
                        expires_at: command.expires_at,
                        audiences: command
                            .audiences
                            .into_iter()
                            .map(|audience| DatabaseAudience {
                                kind: match audience.kind {
                                    MessageAudienceRecordKind::Tenant => {
                                        DatabaseAudienceKind::Tenant
                                    }
                                    MessageAudienceRecordKind::Role => DatabaseAudienceKind::Role,
                                    MessageAudienceRecordKind::User => DatabaseAudienceKind::User,
                                },
                                target_id: audience.target_id,
                            })
                            .collect(),
                    },
                    max_recipients,
                )
                .await?;
            let tenant_id = std::mem::take(&mut published.message.tenant_id);
            Ok(PublishedMessageRecord {
                tenant_id,
                message: to_message_record(published.message),
                recipient_count: published.recipient_count,
                inserted: published.inserted,
            })
        })
    }

    fn record_outbox(&self, event: MessageOutboxRecord) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            let dedupe_key = format!("message:{}", event.aggregate_id);
            OutboxEventRepository
                .record_in_transaction(
                    &self.transaction,
                    RecordOutboxEvent {
                        tenant_id: Some(event.tenant_id),
                        event_type: event.event_type,
                        aggregate_type: "message".into(),
                        aggregate_id: event.aggregate_id,
                        payload: event.payload,
                        available_at: event.available_at,
                        max_attempts: 20,
                        dedupe_key: Some(dedupe_key),
                        traceparent: event.traceparent,
                        tracestate: event.tracestate,
                    },
                    event.available_at,
                )
                .await
                .map(|_| ())
        })
    }

    fn acknowledge<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            MessageRepository
                .acknowledge(&self.transaction, tenant_id, user_id, message_ids, now)
                .await
        })
    }

    fn mark_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            MessageRepository
                .mark_read(&self.transaction, tenant_id, user_id, message_id, now)
                .await
        })
    }

    fn mark_all_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            MessageRepository
                .mark_all_read(&self.transaction, tenant_id, user_id, now)
                .await
        })
    }

    fn soft_delete<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            MessageRepository
                .soft_delete(&self.transaction, tenant_id, user_id, message_ids, now)
                .await
        })
    }

    fn mark_enqueued(
        &self,
        message_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, u64> {
        Box::pin(async move {
            MessageRepository
                .mark_enqueued(&self.transaction, message_id, now)
                .await
        })
    }

    fn delete_expired_batch(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        batch_size: u64,
    ) -> PersistenceFuture<'_, u64> {
        Box::pin(async move {
            MessageRepository
                .delete_expired_batch(&self.transaction, now, batch_size)
                .await
        })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

impl ControlTransaction for DatabaseMessageTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::super::audit_persistence::commit_current_audit(self.transaction).await
        })
    }
}

fn to_recipient_record(record: RecipientMessage) -> MessageRecipientRecord {
    MessageRecipientRecord {
        tenant_id: record.recipient.tenant_id,
        user_id: record.recipient.user_id,
        acked_at: record.recipient.acked_at,
        read_at: record.recipient.read_at,
        message: to_message_record(record.message),
    }
}

fn to_message_record(message: message::Model) -> MessageRecord {
    MessageRecord {
        id: message.id,
        topic: message.topic,
        title_text: message.title_text,
        body_text: message.body_text,
        title_key: message.title_key,
        body_key: message.body_key,
        args_json: message.args_json,
        severity: message.severity,
        payload_json: message.payload_json,
        published_at: message.published_at,
        expires_at: message.expires_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
