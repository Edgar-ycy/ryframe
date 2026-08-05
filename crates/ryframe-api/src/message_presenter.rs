use serde::Serialize;
use utoipa::ToSchema;

use ryframe_http::HttpResult;
use ryframe_i18n::{Locale, LocalizedText, Localizer};
use ryframe_kernel::AppError;
use ryframe_service::system::{MessageInbox, MessageTemplate, MessageText, PublishedMessage};

/// 面向 REST 与 WebSocket 客户端的已渲染消息。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageVo {
    pub id: String,
    pub topic: String,
    pub title: String,
    pub content: String,
    pub severity: String,
    pub payload: Option<serde_json::Value>,
    pub published_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub acked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 面向 REST 客户端的游标收件箱响应。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageInboxPage {
    pub records: Vec<MessageVo>,
    pub next_cursor: Option<String>,
}

/// 面向 REST 客户端的发布结果。
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PublishedMessageVo {
    pub message: MessageVo,
    pub recipient_count: usize,
    pub inserted: bool,
}

/// 将服务层收件箱按当前请求语言转换为传输响应。
pub fn render_inbox(page: MessageInbox, localizer: &Localizer, locale: Locale) -> MessageInboxPage {
    MessageInboxPage {
        records: page
            .records
            .iter()
            .map(|message| render_message(message, localizer, locale))
            .collect(),
        next_cursor: page.next_cursor,
    }
}

/// 将服务层发布结果按当前请求语言转换为传输响应。
pub fn render_published(
    published: PublishedMessage,
    localizer: &Localizer,
    locale: Locale,
) -> PublishedMessageVo {
    PublishedMessageVo {
        message: render_message(&published.message, localizer, locale),
        recipient_count: published.recipient_count,
        inserted: published.inserted,
    }
}

/// 渲染单条消息模板，供 WebSocket 连接和 REST 响应共用。
pub fn render_message(
    message: &MessageTemplate,
    localizer: &Localizer,
    locale: Locale,
) -> MessageVo {
    MessageVo {
        id: message.id.clone(),
        topic: message.topic.clone(),
        title: render_part(
            message.title_text.as_deref(),
            message.title_key.as_deref(),
            &message.args,
            localizer,
            locale,
        ),
        content: render_part(
            message.body_text.as_deref(),
            message.body_key.as_deref(),
            &message.args,
            localizer,
            locale,
        ),
        severity: message.severity.clone(),
        payload: message.payload.clone(),
        published_at: message.published_at,
        expires_at: message.expires_at,
        acked_at: message.acked_at,
        read_at: message.read_at,
    }
}

/// 校验本地化键后转换为不依赖国际化 crate 的服务层表达。
pub fn into_message_text(text: LocalizedText, localizer: &Localizer) -> HttpResult<MessageText> {
    match text {
        LocalizedText::Literal { value } => Ok(MessageText::Literal { value }),
        LocalizedText::Key { key, args } => {
            if !localizer.has_key(&key) {
                return Err(AppError::Validation(format!("消息本地化键不存在: {key}")).into());
            }
            Ok(MessageText::Key { key, args })
        }
    }
}

fn render_part(
    literal: Option<&str>,
    key: Option<&str>,
    args: &std::collections::BTreeMap<String, String>,
    localizer: &Localizer,
    locale: Locale,
) -> String {
    literal
        .map(str::to_owned)
        .or_else(|| key.map(|key| localizer.translate_with_args(locale, key, args)))
        .unwrap_or_default()
}
