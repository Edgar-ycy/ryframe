use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::PersistenceTransaction;

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

#[async_trait::async_trait]
pub trait MessageTransaction: PersistenceTransaction + Sync {
    async fn publish(
        &self,
        command: PublishMessageRecord,
        max_recipients: u64,
    ) -> ryframe_kernel::AppResult<PublishedMessageRecord>;

    async fn record_outbox(&self, event: MessageOutboxRecord) -> ryframe_kernel::AppResult<()>;

    async fn acknowledge<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn mark_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<bool>;

    async fn mark_all_read<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn soft_delete<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        message_ids: &'a [i64],
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn mark_enqueued(
        &self,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn delete_expired_batch(
        &self,
        now: DateTime<Utc>,
        batch_size: u64,
    ) -> ryframe_kernel::AppResult<u64>;
}

#[async_trait::async_trait]
pub trait MessagePersistencePort: Send + Sync {
    async fn inbox<'a>(
        &'a self,
        filter: MessageInboxFilter<'a>,
    ) -> ryframe_kernel::AppResult<MessagePage>;

    async fn unacknowledged_recipients<'a>(
        &'a self,
        message_id: i64,
        user_ids: Option<&'a [i64]>,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<Vec<MessageRecipientRecord>>;

    async fn unread_count<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn find_message(
        &self,
        message_id: i64,
    ) -> ryframe_kernel::AppResult<Option<MessageRecord>>;

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn MessageTransaction>>;
}
