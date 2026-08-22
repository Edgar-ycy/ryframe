use chrono::Duration;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde_json::Value;

use crate::ports::system::{
    MessageAudienceRecord, MessageAudienceRecordKind, MessageOutboxRecord, PublishMessageRecord,
};

use super::{
    MessageAudienceKind, MessageService, MessageTemplate, MessageText, PublishMessageParams,
    PublishedMessage,
};

impl MessageService {
    /// 在单个事务中发布消息、固化收件人并写入投递事件。
    pub async fn publish(
        &self,
        actor: &ActorContext,
        params: PublishMessageParams,
    ) -> AppResult<PublishedMessage> {
        self.ensure_enabled()?;
        let PublishMessageParams {
            tenant_id: requested_tenant_id,
            topic,
            title,
            content,
            severity,
            payload,
            source_type,
            source_id,
            audiences,
            expires_at,
        } = params;
        let actor_tenant_id = crate::validated_tenant_id(actor)?;
        let tenant_id = match requested_tenant_id {
            Some(tenant_id) => {
                crate::enforce_tenant_scope(&tenant_id)?;
                tenant_id
            }
            None => actor_tenant_id.to_owned(),
        };
        let now = self.queue.database_now().await?;
        let maximum_expiry = now + Duration::days(i64::from(self.config.retention_days));
        let expires_at = expires_at.unwrap_or(maximum_expiry);
        if expires_at > maximum_expiry {
            return Err(AppError::Validation(format!(
                "消息有效期不能超过 {} 天",
                self.config.retention_days
            )));
        }
        let content = prepare_content(title, content)?;
        let audiences = audiences
            .into_iter()
            .map(|selector| MessageAudienceRecord {
                kind: match selector.kind {
                    MessageAudienceKind::Tenant => MessageAudienceRecordKind::Tenant,
                    MessageAudienceKind::Role => MessageAudienceRecordKind::Role,
                    MessageAudienceKind::User => MessageAudienceRecordKind::User,
                },
                target_id: selector.target_id,
            })
            .collect();

        let transaction = self.persistence.begin().await?;
        let published = transaction
            .publish(
                PublishMessageRecord {
                    tenant_id,
                    topic,
                    title_text: content.title_text,
                    body_text: content.body_text,
                    title_key: content.title_key,
                    body_key: content.body_key,
                    args_json: content.args_json,
                    severity,
                    payload_json: payload,
                    source_type,
                    source_id,
                    created_by: actor.user_id,
                    published_at: now,
                    expires_at,
                    audiences,
                },
                self.config.max_recipients_per_message,
            )
            .await?;
        if published.inserted {
            let aggregate_id = published.message.id.to_string();
            let trace_context = crate::trace_context::current_trace_context();
            transaction
                .record_outbox(MessageOutboxRecord {
                    tenant_id: published.tenant_id,
                    event_type: crate::jobs::MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE.into(),
                    aggregate_id,
                    payload: serde_json::json!({
                        "message_id": published.message.id.to_string()
                    }),
                    available_at: now,
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                })
                .await?;
        }
        let inserted = published.inserted;
        let recipient_count = published.recipient_count;
        let message = MessageTemplate::from_record(published.message, None, None);
        transaction.commit().await?;
        if inserted {
            self.queue.notify_outbox().await;
        }
        Ok(PublishedMessage {
            message,
            recipient_count,
            inserted,
        })
    }
}

fn prepare_content(title: MessageText, content: MessageText) -> AppResult<PreparedMessageContent> {
    match (title, content) {
        (MessageText::Literal { value: title }, MessageText::Literal { value: content }) => {
            Ok(PreparedMessageContent {
                title_text: Some(title),
                body_text: Some(content),
                title_key: None,
                body_key: None,
                args_json: None,
            })
        }
        (
            MessageText::Key {
                key: title_key,
                args: title_args,
            },
            MessageText::Key {
                key: body_key,
                args: body_args,
            },
        ) => {
            if title_args != body_args {
                return Err(AppError::Validation(
                    "消息标题和正文的本地化参数必须一致".into(),
                ));
            }
            Ok(PreparedMessageContent {
                title_text: None,
                body_text: None,
                title_key: Some(title_key),
                body_key: Some(body_key),
                args_json: (!title_args.is_empty())
                    .then(|| serde_json::to_value(title_args))
                    .transpose()
                    .map_err(|error| {
                        AppError::Validation(format!("消息本地化参数无效: {error}"))
                    })?,
            })
        }
        _ => Err(AppError::Validation(
            "消息标题和正文必须同时使用纯文本或本地化键".into(),
        )),
    }
}

/// 校验标题与正文的本地化表示是否能够组成同一条消息。
pub fn validate_message_text_pair(title: MessageText, content: MessageText) -> AppResult<()> {
    prepare_content(title, content).map(drop)
}

struct PreparedMessageContent {
    title_text: Option<String>,
    body_text: Option<String>,
    title_key: Option<String>,
    body_key: Option<String>,
    args_json: Option<Value>,
}
