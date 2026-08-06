use chrono::{DateTime, Utc};
use ryframe_kernel::AppResult;
use sea_orm::sea_query::{Expr, SelectStatement};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait,
};

use crate::entities::{message, message_recipient};

use super::{
    MessageInboxQuery, MessageRepository, RecipientMessage, RecipientMessagePage, database_error,
};

impl MessageRepository {
    /// 获取用户收件箱；消息 ID 作为稳定的降序游标。
    pub async fn inbox(
        &self,
        db: &DatabaseConnection,
        query: MessageInboxQuery<'_>,
    ) -> AppResult<RecipientMessagePage> {
        let MessageInboxQuery {
            tenant_id,
            user_id,
            cursor,
            limit,
            unread_only,
            unacknowledged_only,
            now,
        } = query;
        let mut recipient_query = message_recipient::Entity::find()
            .find_also_related(message::Entity)
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            // 关联消息的有效性必须在数据库侧过滤，避免大量已过期收件箱记录造成
            // 分页空页或错误结束游标。
            .filter(message::Column::TenantId.eq(tenant_id))
            .filter(message::Column::PublishedAt.lte(now))
            .filter(
                Condition::any()
                    .add(message::Column::ExpiresAt.is_null())
                    .add(message::Column::ExpiresAt.gt(now)),
            );
        if let Some(cursor) = cursor {
            recipient_query =
                recipient_query.filter(message_recipient::Column::MessageId.lt(cursor));
        }
        if unread_only {
            recipient_query = recipient_query.filter(message_recipient::Column::ReadAt.is_null());
        }
        if unacknowledged_only {
            recipient_query = recipient_query.filter(message_recipient::Column::AckedAt.is_null());
        }
        let mut rows = recipient_query
            .order_by_desc(message_recipient::Column::MessageId)
            .limit(limit.saturating_add(1))
            .all(db)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let records = rows
            .into_iter()
            .filter_map(|(recipient, message)| {
                message.map(|message| RecipientMessage { message, recipient })
            })
            .collect::<Vec<_>>();
        Ok(RecipientMessagePage {
            next_cursor: has_more
                .then(|| records.last().map(|record| record.message.id))
                .flatten(),
            records,
        })
    }

    /// 查询一条仍可投递消息的全部未确认收件人，用于在线连接的即时唤醒。
    pub async fn unacknowledged_recipients_for_message(
        &self,
        db: &DatabaseConnection,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<RecipientMessage>> {
        self.unacknowledged_recipients_for_message_for_users(db, message_id, None, now)
            .await
    }

    /// 查询指定在线用户集合中仍需确认的收件人，避免每个 API 实例加载全局收件人快照。
    pub async fn unacknowledged_recipients_for_message_for_online_users(
        &self,
        db: &DatabaseConnection,
        message_id: i64,
        user_ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<Vec<RecipientMessage>> {
        self.unacknowledged_recipients_for_message_for_users(db, message_id, Some(user_ids), now)
            .await
    }

    async fn unacknowledged_recipients_for_message_for_users(
        &self,
        db: &DatabaseConnection,
        message_id: i64,
        user_ids: Option<&[i64]>,
        now: DateTime<Utc>,
    ) -> AppResult<Vec<RecipientMessage>> {
        if user_ids.is_some_and(|ids| ids.is_empty()) {
            return Ok(Vec::new());
        }
        let Some(message) = message::Entity::find_by_id(message_id)
            .filter(message::Column::PublishedAt.lte(now))
            .filter(
                Condition::any()
                    .add(message::Column::ExpiresAt.is_null())
                    .add(message::Column::ExpiresAt.gt(now)),
            )
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(Vec::new());
        };

        let mut recipient_query = message_recipient::Entity::find()
            .filter(message_recipient::Column::MessageId.eq(message.id))
            .filter(message_recipient::Column::TenantId.eq(&message.tenant_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(message_recipient::Column::AckedAt.is_null());
        if let Some(user_ids) = user_ids {
            recipient_query =
                recipient_query.filter(message_recipient::Column::UserId.is_in(user_ids));
        }
        let recipients = recipient_query.all(db).await.map_err(database_error)?;

        Ok(recipients
            .into_iter()
            .map(|recipient| RecipientMessage {
                message: message.clone(),
                recipient,
            })
            .collect())
    }

    /// 返回未读消息数量。
    pub async fn unread_count(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64> {
        message_recipient::Entity::find()
            .inner_join(message::Entity)
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(message_recipient::Column::ReadAt.is_null())
            .filter(message::Column::TenantId.eq(tenant_id))
            .filter(message::Column::PublishedAt.lte(now))
            .filter(
                Condition::any()
                    .add(message::Column::ExpiresAt.is_null())
                    .add(message::Column::ExpiresAt.gt(now)),
            )
            .count(db)
            .await
            .map_err(database_error)
    }

    /// 批量确认用户已收到指定消息。
    pub async fn acknowledge<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        if message_ids.is_empty() {
            return Ok(0);
        }
        let result = message_recipient::Entity::update_many()
            .col_expr(message_recipient::Column::AckedAt, Expr::value(now))
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.is_in(message_ids.to_vec()))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(message_recipient::Column::AckedAt.is_null())
            .filter(
                message_recipient::Column::MessageId
                    .in_subquery(active_message_id_subquery(tenant_id, now)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }

    /// 记录消息首次进入实时投递阶段的时间，不覆盖已有值，便于审计投递延迟。
    pub async fn mark_enqueued<C>(
        &self,
        db: &C,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        let result = message_recipient::Entity::update_many()
            .col_expr(message_recipient::Column::EnqueuedAt, Expr::value(now))
            .filter(message_recipient::Column::MessageId.eq(message_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(message_recipient::Column::EnqueuedAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }

    /// 标记单条消息为已读，同时确认已投递。
    pub async fn mark_read<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_id: i64,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::ReadAt,
                Expr::cust_with_values("COALESCE(`read_at`, ?)", [now]),
            )
            .col_expr(
                message_recipient::Column::AckedAt,
                Expr::cust_with_values("COALESCE(`acked_at`, ?)", [now]),
            )
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.eq(message_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(
                message_recipient::Column::MessageId
                    .in_subquery(active_message_id_subquery(tenant_id, now)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        if result.rows_affected > 0 {
            return Ok(true);
        }

        // MySQL 在默认的“实际变更行数”语义下，会把重复写入同一已读状态报告为零行。
        // 再次按同一可见性边界查询，保证重复的已读请求仍保持幂等，同时不放宽租户、用户、
        // 软删除或过期消息的隔离条件。
        let existing = message_recipient::Entity::find()
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.eq(message_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(
                message_recipient::Column::MessageId
                    .in_subquery(active_message_id_subquery(tenant_id, now)),
            )
            .one(db)
            .await
            .map_err(database_error)?;
        Ok(existing.is_some())
    }

    /// 将当前用户的全部未读消息标记为已读。
    pub async fn mark_all_read<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::ReadAt,
                Expr::cust_with_values("COALESCE(`read_at`, ?)", [now]),
            )
            .col_expr(
                message_recipient::Column::AckedAt,
                Expr::cust_with_values("COALESCE(`acked_at`, ?)", [now]),
            )
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .filter(message_recipient::Column::ReadAt.is_null())
            .filter(
                message_recipient::Column::MessageId
                    .in_subquery(active_message_id_subquery(tenant_id, now)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }

    /// 软删除当前用户收到的消息；重复删除和越权 ID 均按幂等操作处理。
    pub async fn soft_delete<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        if message_ids.is_empty() {
            return Ok(0);
        }
        let result = message_recipient::Entity::update_many()
            .col_expr(message_recipient::Column::DeletedAt, Expr::value(now))
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.is_in(message_ids.to_vec()))
            .filter(message_recipient::Column::DeletedAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }

    /// 删除一批到期消息；关联的受众和收件箱记录由外键级联删除。
    pub async fn delete_expired_batch<C>(
        &self,
        db: &C,
        now: DateTime<Utc>,
        batch_size: u64,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        let ids = message::Entity::find()
            .filter(message::Column::ExpiresAt.lte(now))
            .order_by_asc(message::Column::Id)
            .limit(batch_size.max(1))
            .select_only()
            .column(message::Column::Id)
            .into_tuple::<i64>()
            .all(db)
            .await
            .map_err(database_error)?;
        if ids.is_empty() {
            return Ok(0);
        }
        let result = message::Entity::delete_many()
            .filter(message::Column::Id.is_in(ids))
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }
}

fn active_message_id_subquery(tenant_id: &str, now: DateTime<Utc>) -> SelectStatement {
    message::Entity::find()
        .select_only()
        .column(message::Column::Id)
        .filter(message::Column::TenantId.eq(tenant_id))
        .filter(message::Column::PublishedAt.lte(now))
        .filter(
            Condition::any()
                .add(message::Column::ExpiresAt.is_null())
                .add(message::Column::ExpiresAt.gt(now)),
        )
        .into_query()
}
