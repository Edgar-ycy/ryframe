use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Duration, Utc};
use ryframe_config::MessagingConfig;
use ryframe_db::{
    DatabaseCluster, MessageInboxQuery, MessageRepository, OutboxEventRepository,
    PublishMessageCommand, RecipientMessage, RecordOutboxEvent,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{EntityTrait, TransactionTrait};
use serde_json::Value;

use crate::JobQueue;

/// 消息投递任务的稳定类型标识。
pub const MESSAGE_DISPATCH_JOB_TYPE: &str = "system.message.dispatch";
/// 供 API 实例订阅的跨实例消息唤醒频道。
pub const MESSAGE_DISPATCH_REDIS_CHANNEL: &str = "ryframe:message:dispatch";
/// 每日清理过期消息的稳定任务类型标识。
pub const MESSAGE_RETENTION_JOB_TYPE: &str = "system.message.retention";
const RETENTION_BATCH_SIZE: u64 = 500;

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

/// MySQL 持久化消息中心服务。
pub struct MessageService {
    db: DatabaseCluster,
    repository: MessageRepository,
    outbox: OutboxEventRepository,
    queue: Arc<JobQueue>,
    config: MessagingConfig,
}

impl MessageService {
    /// 使用主库和持久化任务队列构造服务。
    pub fn new(db: DatabaseCluster, queue: Arc<JobQueue>, config: MessagingConfig) -> Self {
        Self {
            db,
            repository: MessageRepository,
            outbox: OutboxEventRepository,
            queue,
            config,
        }
    }

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

        Ok(PublishedMessage {
            message: MessageTemplate::from_published(&published.message),
            recipient_count: published.recipient_count,
            inserted: published.inserted,
        })
    }

    /// 读取当前用户的收件箱。
    pub async fn inbox(
        &self,
        actor: &ActorContext,
        cursor: Option<i64>,
        limit: u64,
        unread_only: bool,
    ) -> AppResult<MessageInbox> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let page = self
            .repository
            .inbox(
                self.db.write(),
                MessageInboxQuery {
                    tenant_id,
                    user_id: actor.user_id,
                    cursor,
                    limit,
                    unread_only,
                    unacknowledged_only: false,
                    now,
                },
            )
            .await?;
        Ok(MessageInbox::from_page(page))
    }

    /// 按已通过一次性票据验证的身份补拉尚未确认的消息。
    ///
    /// 该入口只接受服务器从票据恢复出的租户和用户，不接受客户端可伪造的身份字段。
    pub async fn unacknowledged_for_identity(
        &self,
        tenant_id: &str,
        user_id: i64,
        limit: u64,
    ) -> AppResult<MessageInbox> {
        self.ensure_enabled()?;
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        let page = self
            .repository
            .inbox(
                self.db.write(),
                MessageInboxQuery {
                    tenant_id,
                    user_id,
                    cursor: None,
                    limit,
                    unread_only: false,
                    unacknowledged_only: true,
                    now: self.queue.database_now().await?,
                },
            )
            .await?;
        Ok(MessageInbox::from_page(page))
    }

    /// 按已通过一次性票据验证的身份确认消息已经送达。
    pub async fn acknowledge_for_identity(
        &self,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
    ) -> AppResult<u64> {
        self.ensure_enabled()?;
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let updated = self
            .repository
            .acknowledge(&transaction, tenant_id, user_id, message_ids, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(updated)
    }

    /// 返回仍需要向在线连接唤醒的收件人快照。
    pub async fn unacknowledged_recipients_for_message(
        &self,
        message_id: i64,
    ) -> AppResult<Vec<MessageDelivery>> {
        self.ensure_enabled()?;
        self.repository
            .unacknowledged_recipients_for_message(
                self.db.write(),
                message_id,
                self.queue.database_now().await?,
            )
            .await
            .map(|records| {
                records
                    .iter()
                    .map(MessageDelivery::from_recipient)
                    .collect()
            })
    }

    /// 返回本 API 实例在线用户中仍需要唤醒的收件人。
    ///
    /// Redis Pub/Sub 会向每个实例广播消息标识，因此必须在数据库查询中限制为本实例
    /// 的在线用户，避免每个实例重复加载所有收件人快照。
    pub async fn unacknowledged_recipients_for_online_users(
        &self,
        message_id: i64,
        user_ids: &[i64],
    ) -> AppResult<Vec<MessageDelivery>> {
        self.ensure_enabled()?;
        self.repository
            .unacknowledged_recipients_for_message_for_online_users(
                self.db.write(),
                message_id,
                user_ids,
                self.queue.database_now().await?,
            )
            .await
            .map(|records| {
                records
                    .iter()
                    .map(MessageDelivery::from_recipient)
                    .collect()
            })
    }

    /// 统计当前用户的未读消息数量。
    pub async fn unread_count(&self, actor: &ActorContext) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.repository
            .unread_count(
                self.db.write(),
                tenant_id,
                actor.user_id,
                self.queue.database_now().await?,
            )
            .await
    }

    /// 确认客户端已接收的消息。
    pub async fn acknowledge(&self, actor: &ActorContext, message_ids: &[i64]) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let updated = self
            .repository
            .acknowledge(&transaction, tenant_id, actor.user_id, message_ids, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(updated)
    }

    /// 将单条消息标记为已读。
    pub async fn mark_read(&self, actor: &ActorContext, message_id: i64) -> AppResult<()> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let updated = self
            .repository
            .mark_read(&transaction, tenant_id, actor.user_id, message_id, now)
            .await?;
        if updated {
            crate::commit_current_audit(transaction).await?;
            Ok(())
        } else {
            let _ = transaction.rollback().await;
            Err(AppError::NotFound("消息不存在或不属于当前用户".into()))
        }
    }

    /// 将当前用户全部未读消息标记为已读。
    pub async fn mark_all_read(&self, actor: &ActorContext) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let updated = self
            .repository
            .mark_all_read(&transaction, tenant_id, actor.user_id, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(updated)
    }

    /// 完成投递任务的持久化阶段。API 实例可据此拉取收件箱并即时发送。
    pub async fn dispatch(&self, message_id: i64) -> AppResult<()> {
        self.ensure_enabled()?;
        let now = self.queue.database_now().await?;
        let message = ryframe_db::message::Entity::find_by_id(message_id)
            .one(self.db.write())
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("待投递消息不存在".into()))?;
        if message
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Ok(());
        }
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        self.repository
            .mark_enqueued(&transaction, message_id, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        // 当前实现以持久化收件箱为准；WebSocket 或 Redis 唤醒失败后，客户端仍可轮询补拉。
        Ok(())
    }

    /// 删除已到期的消息及其级联收件箱记录。
    pub async fn delete_expired(&self) -> AppResult<u64> {
        self.ensure_enabled()?;
        let now = self.queue.database_now().await?;
        let mut deleted = 0_u64;
        loop {
            let transaction = self.db.write().begin().await.map_err(database_error)?;
            let batch = self
                .repository
                .delete_expired_batch(&transaction, now, RETENTION_BATCH_SIZE)
                .await?;
            crate::commit_current_audit(transaction).await?;
            deleted = deleted.saturating_add(batch);
            if batch < RETENTION_BATCH_SIZE {
                return Ok(deleted);
            }
        }
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

    fn ensure_enabled(&self) -> AppResult<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("消息中心已关闭".into()))
        }
    }
}

struct PreparedMessageContent {
    title_text: Option<String>,
    body_text: Option<String>,
    title_key: Option<String>,
    body_key: Option<String>,
    args_json: Option<Value>,
}

impl MessageTemplate {
    fn from_published(message: &ryframe_db::message::Model) -> Self {
        Self::from_parts(message, None, None)
    }

    fn from_recipient(record: &RecipientMessage) -> Self {
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
    fn from_page(page: ryframe_db::RecipientMessagePage) -> Self {
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
    fn from_recipient(record: &RecipientMessage) -> Self {
        Self {
            tenant_id: record.recipient.tenant_id.clone(),
            user_id: record.recipient.user_id,
            message: MessageTemplate::from_recipient(record),
        }
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
