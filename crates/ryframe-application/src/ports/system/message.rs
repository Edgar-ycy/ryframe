use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{ControlTransaction, PersistenceFuture};

#[derive(Debug)]
pub struct MessageRecord {
    pub id: i64,
    pub topic: String,
    pub title_text: Option<String>,
    pub body_text: Option<String>,
    pub title_key: Option<String>,
    pub body_key: Option<String>,
    pub args_json: Option<Value>,
    pub severity: String,
    pub payload_json: Option<Value>,
    pub published_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub struct MessageRecipientRecord {
    pub tenant_id: String,
    pub user_id: i64,
    pub acked_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub message: MessageRecord,
}

#[derive(Debug)]
pub struct MessagePage {
    pub records: Vec<MessageRecipientRecord>,
    pub next_cursor: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MessageAudienceRecordKind {
    Tenant,
    Role,
    User,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MessageAudienceRecord {
    pub kind: MessageAudienceRecordKind,
    pub target_id: i64,
}

#[derive(Debug)]
pub struct PublishMessageRecord {
    pub tenant_id: String,
    pub topic: String,
    pub title_text: Option<String>,
    pub body_text: Option<String>,
    pub title_key: Option<String>,
    pub body_key: Option<String>,
    pub args_json: Option<Value>,
    pub severity: String,
    pub payload_json: Option<Value>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub created_by: i64,
    pub published_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub audiences: Vec<MessageAudienceRecord>,
}

#[derive(Debug)]
pub struct PublishedMessageRecord {
    pub tenant_id: String,
    pub message: MessageRecord,
    pub recipient_count: usize,
    pub inserted: bool,
}

#[derive(Debug)]
pub struct MessageOutboxRecord {
    pub tenant_id: String,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub available_at: DateTime<Utc>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct MessageInboxFilter<'a> {
    pub tenant_id: &'a str,
    pub user_id: i64,
    pub cursor: Option<i64>,
    pub limit: u64,
    pub unread_only: bool,
    pub unacknowledged_only: bool,
    pub now: DateTime<Utc>,
}

pub trait MessageTransaction: ControlTransaction + Sync {
    fn publish(
        &self,
        command: PublishMessageRecord,
        max_recipients: u64,
    ) -> PersistenceFuture<'_, PublishedMessageRecord>;

    fn record_outbox(&self, event: MessageOutboxRecord) -> PersistenceFuture<'_, ()>;

    fn acknowledge<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn mark_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, bool>;

    fn mark_all_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn soft_delete<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn mark_enqueued(&self, message_id: i64, now: DateTime<Utc>) -> PersistenceFuture<'_, u64>;

    fn delete_expired_batch(
        &self,
        now: DateTime<Utc>,
        batch_size: u64,
    ) -> PersistenceFuture<'_, u64>;

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()>;
}

pub trait MessagePersistencePort: Send + Sync {
    fn inbox<'a>(&'a self, filter: MessageInboxFilter<'a>) -> PersistenceFuture<'a, MessagePage>;

    fn unacknowledged_recipients<'a>(
        &'a self,
        message_id: i64,
        user_ids: Option<&'a [i64]>,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, Vec<MessageRecipientRecord>>;

    fn unread_count<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> PersistenceFuture<'a, u64>;

    fn find_message(&self, message_id: i64) -> PersistenceFuture<'_, Option<MessageRecord>>;

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn MessageTransaction>>;
}
