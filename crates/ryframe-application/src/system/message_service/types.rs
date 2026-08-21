use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::ports::system::{MessagePage, MessageRecipientRecord, MessageRecord};

/// 创建消息的业务参数。
#[derive(Debug)]
pub struct PublishMessageParams {
    /// 可选的目标租户；调用方必须在传入前完成跨租户授权。
    pub tenant_id: Option<String>,
    pub topic: String,
    pub title: MessageText,
    pub content: MessageText,
    pub severity: String,
    pub payload: Option<Value>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub audiences: Vec<MessageAudienceSelector>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// 纯文本或本地化键形式的消息片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageText {
    Literal {
        value: String,
    },
    Key {
        key: String,
        args: BTreeMap<String, String>,
    },
}

/// 消息受众的稳定业务类型。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MessageAudienceKind {
    Tenant,
    Role,
    User,
}

/// 发布消息时的单个受众选择器。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MessageAudienceSelector {
    pub kind: MessageAudienceKind,
    pub target_id: i64,
}

/// 服务层返回的未渲染消息模板。
///
/// HTTP 与 WebSocket 适配层必须使用各自连接协商出的语言渲染本模板，
/// 不能把原始本地化键或参数直接暴露给客户端。
#[derive(Debug, Clone)]
pub struct MessageTemplate {
    pub id: String,
    pub topic: String,
    pub title_text: Option<String>,
    pub body_text: Option<String>,
    pub title_key: Option<String>,
    pub body_key: Option<String>,
    pub args: BTreeMap<String, String>,
    pub severity: String,
    pub payload: Option<Value>,
    pub published_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub acked_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
}

/// 游标收件箱的服务层结果。
#[derive(Debug, Clone)]
pub struct MessageInbox {
    pub records: Vec<MessageTemplate>,
    pub next_cursor: Option<String>,
}

/// 发布消息后的服务层摘要。
#[derive(Debug, Clone)]
pub struct PublishedMessage {
    pub message: MessageTemplate,
    pub recipient_count: usize,
    pub inserted: bool,
}

/// 供传输适配层按连接语言渲染并投递的收件人记录。
#[derive(Debug, Clone)]
pub struct MessageDelivery {
    pub tenant_id: String,
    pub user_id: i64,
    pub message: MessageTemplate,
}

impl MessageTemplate {
    pub(super) fn from_record(
        message: MessageRecord,
        acked_at: Option<DateTime<Utc>>,
        read_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: message.id.to_string(),
            topic: message.topic,
            title_text: message.title_text,
            body_text: message.body_text,
            title_key: message.title_key,
            body_key: message.body_key,
            args: message
                .args_json
                .and_then(|value| serde_json::from_value::<BTreeMap<String, String>>(value).ok())
                .unwrap_or_default(),
            severity: message.severity,
            payload: message.payload_json,
            published_at: message.published_at,
            expires_at: message.expires_at,
            acked_at,
            read_at,
        }
    }
}

impl MessageInbox {
    pub(super) fn from_page(page: MessagePage) -> Self {
        Self {
            records: page
                .records
                .into_iter()
                .map(|record| {
                    MessageTemplate::from_record(record.message, record.acked_at, record.read_at)
                })
                .collect(),
            next_cursor: page.next_cursor.map(|cursor| cursor.to_string()),
        }
    }
}

impl MessageDelivery {
    pub(super) fn from_recipient(record: MessageRecipientRecord) -> Self {
        Self {
            tenant_id: record.tenant_id,
            user_id: record.user_id,
            message: MessageTemplate::from_record(record.message, record.acked_at, record.read_at),
        }
    }
}
