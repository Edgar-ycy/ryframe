use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_db::RecipientMessage;
use serde_json::Value;

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
    pub(super) fn from_published(message: &ryframe_db::message::Model) -> Self {
        Self::from_parts(message, None, None)
    }

    pub(super) fn from_recipient(record: &RecipientMessage) -> Self {
        Self::from_parts(
            &record.message,
            record.recipient.acked_at,
            record.recipient.read_at,
        )
    }

    fn from_parts(
        message: &ryframe_db::message::Model,
        acked_at: Option<DateTime<Utc>>,
        read_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: message.id.to_string(),
            topic: message.topic.clone(),
            title_text: message.title_text.clone(),
            body_text: message.body_text.clone(),
            title_key: message.title_key.clone(),
            body_key: message.body_key.clone(),
            args: message
                .args_json
                .as_ref()
                .and_then(|value| {
                    serde_json::from_value::<BTreeMap<String, String>>(value.clone()).ok()
                })
                .unwrap_or_default(),
            severity: message.severity.clone(),
            payload: message.payload_json.clone(),
            published_at: message.published_at,
            expires_at: message.expires_at,
            acked_at,
            read_at,
        }
    }
}

impl MessageInbox {
    pub(super) fn from_page(page: ryframe_db::RecipientMessagePage) -> Self {
        Self {
            records: page
                .records
                .iter()
                .map(MessageTemplate::from_recipient)
                .collect(),
            next_cursor: page.next_cursor.map(|cursor| cursor.to_string()),
        }
    }
}

impl MessageDelivery {
    pub(super) fn from_recipient(record: &RecipientMessage) -> Self {
        Self {
            tenant_id: record.recipient.tenant_id.clone(),
            user_id: record.recipient.user_id,
            message: MessageTemplate::from_recipient(record),
        }
    }
}
