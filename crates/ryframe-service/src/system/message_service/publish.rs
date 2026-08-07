use chrono::Duration;
use ryframe_db::{PublishMessageCommand, RecordOutboxEvent};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde_json::Value;

use super::{
    MessageAudienceKind, MessageService, MessageTemplate, MessageText, PublishMessageParams,
    PublishedMessage, database_error,
};

impl MessageService {
    /// 在单个事务中发布消息、固化收件人并写入投递任务。
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
        let tenant_id = match requested_tenant_id.as_deref() {
            Some(tenant_id) => {
                ryframe_core::validate_explicit_tenant(tenant_id)?;
                tenant_id.to_owned()
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
        let content = self.prepare_content(&title, &content)?;
        let audiences = audiences
            .into_iter()
            .map(|selector| ryframe_db::MessageAudienceSelector {
                kind: match selector.kind {
                    MessageAudienceKind::Tenant => ryframe_db::MessageAudienceKind::Tenant,
                    MessageAudienceKind::Role => ryframe_db::MessageAudienceKind::Role,
                    MessageAudienceKind::User => ryframe_db::MessageAudienceKind::User,
                },
                target_id: selector.target_id,
            })
            .collect();

        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let published = self
            .repository
            .publish_in_transaction(
                &transaction,
                PublishMessageCommand {
                    tenant_id: tenant_id.clone(),
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
            let payload = serde_json::json!({ "message_id": published.message.id.to_string() });
            let trace_context = crate::trace_context::current_trace_context();
            self.outbox
                .record_in_transaction(
                    &transaction,
                    RecordOutboxEvent {
                        tenant_id: Some(tenant_id.clone()),
                        event_type: "system.message.published".into(),
                        aggregate_type: "message".into(),
                        aggregate_id: published.message.id.to_string(),
                        payload: payload.clone(),
                        available_at: now,
                        max_attempts: 20,
                        dedupe_key: Some(format!("message:{}", published.message.id)),
                        traceparent: trace_context.traceparent,
                        tracestate: trace_context.tracestate,
                    },
                    now,
                )
                .await?;
        }
        crate::commit_current_audit(transaction).await?;
        if published.inserted {
            self.queue.notify_outbox().await;
        }

        Ok(PublishedMessage {
            message: MessageTemplate::from_published(&published.message),
            recipient_count: published.recipient_count,
            inserted: published.inserted,
        })
    }

    fn prepare_content(
        &self,
        title: &MessageText,
        content: &MessageText,
    ) -> AppResult<PreparedMessageContent> {
        match (title, content) {
            (MessageText::Literal { value: title }, MessageText::Literal { value: content }) => {
                Ok(PreparedMessageContent {
                    title_text: Some(title.clone()),
                    body_text: Some(content.clone()),
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
                    title_key: Some(title_key.clone()),
                    body_key: Some(body_key.clone()),
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
}

pub(super) struct PreparedMessageContent {
    title_text: Option<String>,
    body_text: Option<String>,
    title_key: Option<String>,
    body_key: Option<String>,
    args_json: Option<Value>,
}
