use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::entities::{message, message_recipient};

/// 消息受众类型。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MessageAudienceKind {
    Tenant,
    Role,
    User,
}

impl MessageAudienceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::Role => "role",
            Self::User => "user",
        }
    }
}

/// 发布消息时的单个受众选择器。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MessageAudienceSelector {
    pub kind: MessageAudienceKind,
    pub target_id: i64,
}

/// 发布消息的持久化输入。
#[derive(Clone, Debug)]
pub struct PublishMessageCommand {
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
    pub audiences: Vec<MessageAudienceSelector>,
}

/// 发布成功后返回的消息及其收件人数量。
#[derive(Clone, Debug)]
pub struct PublishedMessage {
    pub message: message::Model,
    pub recipient_count: usize,
    pub inserted: bool,
}

/// 收件箱中的消息及用户状态。
#[derive(Clone, Debug)]
pub struct RecipientMessage {
    pub message: message::Model,
    pub recipient: message_recipient::Model,
}

/// 游标分页结果。
#[derive(Clone, Debug)]
pub struct RecipientMessagePage {
    pub records: Vec<RecipientMessage>,
    pub next_cursor: Option<i64>,
}

/// 收件箱查询的边界条件。
#[derive(Clone, Debug)]
pub struct MessageInboxQuery<'a> {
    pub tenant_id: &'a str,
    pub user_id: i64,
    pub cursor: Option<i64>,
    pub limit: u64,
    pub unread_only: bool,
    pub unacknowledged_only: bool,
    pub now: DateTime<Utc>,
}
