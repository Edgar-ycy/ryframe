use std::collections::BTreeSet;

use ryframe_kernel::{ActorContext, AppError, AppResult};

use crate::MessageInboxFilter;

use super::{MessageDelivery, MessageInbox, MessageService};

impl MessageService {
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
        self.persistence
            .inbox(MessageInboxFilter {
                tenant_id,
                user_id: actor.user_id,
                cursor,
                limit,
                unread_only,
                unacknowledged_only: false,
                now: self.queue.database_now().await?,
            })
            .await
            .map(MessageInbox::from_page)
    }

    /// 按已通过一次性票据验证的身份补拉尚未确认的消息。
    pub async fn unacknowledged_for_identity(
        &self,
        tenant_id: &str,
        user_id: i64,
        limit: u64,
    ) -> AppResult<MessageInbox> {
        self.ensure_enabled()?;
        crate::enforce_tenant_scope(tenant_id)?;
        self.persistence
            .inbox(MessageInboxFilter {
                tenant_id,
                user_id,
                cursor: None,
                limit,
                unread_only: false,
                unacknowledged_only: true,
                now: self.queue.database_now().await?,
            })
            .await
            .map(MessageInbox::from_page)
    }

    /// 按已通过一次性票据验证的身份确认消息已经送达。
    pub async fn acknowledge_for_identity(
        &self,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
    ) -> AppResult<u64> {
        self.ensure_enabled()?;
        crate::enforce_tenant_scope(tenant_id)?;
        self.acknowledge_for(tenant_id, user_id, message_ids).await
    }

    /// 返回仍需要向在线连接唤醒的收件人快照。
    pub async fn unacknowledged_recipients_for_message(
        &self,
        message_id: i64,
    ) -> AppResult<Vec<MessageDelivery>> {
        self.ensure_enabled()?;
        self.persistence
            .unacknowledged_recipients(message_id, None, self.queue.database_now().await?)
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(MessageDelivery::from_recipient)
                    .collect()
            })
    }

    /// 返回本 API 实例在线用户中仍需要唤醒的收件人。
    pub async fn unacknowledged_recipients_for_online_users(
        &self,
        message_id: i64,
        user_ids: &[i64],
    ) -> AppResult<Vec<MessageDelivery>> {
        self.ensure_enabled()?;
        self.persistence
            .unacknowledged_recipients(message_id, Some(user_ids), self.queue.database_now().await?)
            .await
            .map(|records| {
                records
                    .into_iter()
                    .map(MessageDelivery::from_recipient)
                    .collect()
            })
    }

    /// 统计当前用户的未读消息数量。
    pub async fn unread_count(&self, actor: &ActorContext) -> AppResult<u64> {
        self.ensure_enabled()?;
        self.persistence
            .unread_count(
                crate::validated_tenant_id(actor)?,
                actor.user_id,
                self.queue.database_now().await?,
            )
            .await
    }

    /// 确认客户端已接收的消息。
    pub async fn acknowledge(&self, actor: &ActorContext, message_ids: &[i64]) -> AppResult<u64> {
        self.ensure_enabled()?;
        self.acknowledge_for(
            crate::validated_tenant_id(actor)?,
            actor.user_id,
            message_ids,
        )
        .await
    }

    async fn acknowledge_for(
        &self,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
    ) -> AppResult<u64> {
        let now = self.queue.database_now().await?;
        let transaction = self.persistence.begin().await?;
        let updated = transaction
            .acknowledge(tenant_id, user_id, message_ids, now)
            .await?;
        transaction.commit().await?;
        Ok(updated)
    }

    /// 将单条消息标记为已读。
    pub async fn mark_read(&self, actor: &ActorContext, message_id: i64) -> AppResult<()> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let transaction = self.persistence.begin().await?;
        if transaction
            .mark_read(tenant_id, actor.user_id, message_id, now)
            .await?
        {
            transaction.commit().await
        } else {
            transaction.rollback().await?;
            Err(AppError::NotFound("消息不存在或不属于当前用户".into()))
        }
    }

    /// 将当前用户全部未读消息标记为已读。
    pub async fn mark_all_read(&self, actor: &ActorContext) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let now = self.queue.database_now().await?;
        let transaction = self.persistence.begin().await?;
        let updated = transaction
            .mark_all_read(tenant_id, actor.user_id, now)
            .await?;
        transaction.commit().await?;
        Ok(updated)
    }

    /// 软删除当前用户收到的消息，不影响其他收件人和消息主记录。
    pub async fn delete(&self, actor: &ActorContext, message_ids: &[i64]) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let message_ids = message_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let now = self.queue.database_now().await?;
        let transaction = self.persistence.begin().await?;
        let deleted = transaction
            .soft_delete(tenant_id, actor.user_id, &message_ids, now)
            .await?;
        transaction.commit().await?;
        Ok(deleted)
    }

    /// 完成投递任务的持久化阶段。
    pub async fn dispatch(&self, message_id: i64) -> AppResult<()> {
        self.ensure_enabled()?;
        let now = self.queue.database_now().await?;
        let message = self
            .persistence
            .find_message(message_id)
            .await?
            .ok_or_else(|| AppError::NotFound("待投递消息不存在".into()))?;
        if message
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Ok(());
        }
        let transaction = self.persistence.begin().await?;
        transaction.mark_enqueued(message_id, now).await?;
        transaction.commit().await
    }
}
