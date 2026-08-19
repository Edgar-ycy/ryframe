use std::collections::BTreeSet;

use ryframe_db::MessageInboxQuery;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{EntityTrait, TransactionTrait};

use super::{MessageDelivery, MessageInbox, MessageService, database_error};

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
        crate::enforce_tenant_scope(tenant_id)?;
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
        crate::enforce_tenant_scope(tenant_id)?;
        self.acknowledge_for(tenant_id, user_id, message_ids).await
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
        self.acknowledge_for(tenant_id, actor.user_id, message_ids)
            .await
    }

    async fn acknowledge_for(
        &self,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
    ) -> AppResult<u64> {
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let updated = self
            .repository
            .acknowledge(&transaction, tenant_id, user_id, message_ids, now)
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

    /// 软删除当前用户收到的消息，不影响其他收件人和消息主记录。
    pub async fn delete(&self, actor: &ActorContext, message_ids: &[i64]) -> AppResult<u64> {
        self.ensure_enabled()?;
        let tenant_id = crate::validated_tenant_id(actor)?;
        let message_ids = message_ids.iter().copied().collect::<BTreeSet<_>>();
        let message_ids = message_ids.into_iter().collect::<Vec<_>>();
        let now = self.queue.database_now().await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let deleted = self
            .repository
            .soft_delete(&transaction, tenant_id, actor.user_id, &message_ids, now)
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(deleted)
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
}
