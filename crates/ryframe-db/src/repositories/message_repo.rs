use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::sea_query::SelectStatement;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, DatabaseConnection,
    DatabaseTransaction, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    QueryTrait,
};
use serde_json::Value;

use crate::entities::{message, message_audience, message_recipient, role, user, user_role};

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

/// 消息中心仓储。
pub struct MessageRepository;

impl MessageRepository {
    /// 在调用方事务中插入消息、受众快照和收件箱快照。
    pub async fn publish_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: PublishMessageCommand,
    ) -> AppResult<PublishedMessage> {
        validate_publish_command(&command)?;
        let audiences = command.audiences.iter().cloned().collect::<BTreeSet<_>>();
        let model = message::Model {
            id: snowflake::try_next_snowflake_id()?,
            tenant_id: command.tenant_id.clone(),
            topic: command.topic.clone(),
            title_text: command.title_text.clone(),
            body_text: command.body_text.clone(),
            title_key: command.title_key.clone(),
            body_key: command.body_key.clone(),
            args_json: command.args_json.clone(),
            severity: command.severity.clone(),
            payload_json: command.payload_json.clone(),
            source_type: command.source_type.clone(),
            source_id: command.source_id.clone(),
            created_by: Some(command.created_by),
            published_at: command.published_at,
            expires_at: Some(command.expires_at),
            created_at: command.published_at,
            updated_at: command.published_at,
        };

        let inserted = match message::ActiveModel::from(model.clone())
            .insert(transaction)
            .await
        {
            Ok(inserted) => inserted,
            Err(error) if is_duplicate_key_error(&error) => {
                let (Some(source_type), Some(source_id)) =
                    (command.source_type.as_deref(), command.source_id.as_deref())
                else {
                    return Err(AppError::Database("消息主键冲突但未提供业务幂等键".into()));
                };
                let existing = message::Entity::find()
                    .filter(message::Column::TenantId.eq(&command.tenant_id))
                    .filter(message::Column::SourceType.eq(source_type))
                    .filter(message::Column::SourceId.eq(source_id))
                    .one(transaction)
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| AppError::Database("消息幂等键冲突后未读取到已有消息".into()))?;
                let recipient_count = message_recipient::Entity::find()
                    .filter(message_recipient::Column::MessageId.eq(existing.id))
                    .count(transaction)
                    .await
                    .map_err(database_error)? as usize;
                return Ok(PublishedMessage {
                    message: existing,
                    recipient_count,
                    inserted: false,
                });
            }
            Err(error) => return Err(database_error(error)),
        };

        let audience_models = audiences
            .iter()
            .map(|selector| message_audience::ActiveModel {
                message_id: Set(inserted.id),
                tenant_id: Set(inserted.tenant_id.clone()),
                kind: Set(selector.kind.as_str().to_owned()),
                target_id: Set(selector.target_id),
            })
            .collect::<Vec<_>>();
        message_audience::Entity::insert_many(audience_models)
            .exec(transaction)
            .await
            .map_err(database_error)?;

        let recipients = self
            .resolve_recipients(transaction, &inserted.tenant_id, &audiences)
            .await?;
        if recipients.is_empty() {
            return Err(AppError::Validation(
                "消息受众中没有可投递的启用用户".into(),
            ));
        }
        let recipient_models = recipients
            .iter()
            .map(|user_id| message_recipient::ActiveModel {
                message_id: Set(inserted.id),
                user_id: Set(*user_id),
                tenant_id: Set(inserted.tenant_id.clone()),
                created_at: Set(inserted.published_at),
                enqueued_at: Set(None),
                acked_at: Set(None),
                read_at: Set(None),
            })
            .collect::<Vec<_>>();
        message_recipient::Entity::insert_many(recipient_models)
            .exec(transaction)
            .await
            .map_err(database_error)?;

        Ok(PublishedMessage {
            message: inserted,
            recipient_count: recipients.len(),
            inserted: true,
        })
    }

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
    pub async fn acknowledge(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        message_ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<u64> {
        if message_ids.is_empty() {
            return Ok(0);
        }
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::AckedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.is_in(message_ids.to_vec()))
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
    pub async fn mark_enqueued(
        &self,
        db: &DatabaseConnection,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64> {
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::EnqueuedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(message_recipient::Column::MessageId.eq(message_id))
            .filter(message_recipient::Column::EnqueuedAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected)
    }

    /// 标记单条消息为已读，同时确认已投递。
    pub async fn mark_read(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        message_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::ReadAt,
                sea_orm::sea_query::Expr::cust_with_values("COALESCE(`read_at`, ?)", [now]),
            )
            .col_expr(
                message_recipient::Column::AckedAt,
                sea_orm::sea_query::Expr::cust_with_values("COALESCE(`acked_at`, ?)", [now]),
            )
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
            .filter(message_recipient::Column::MessageId.eq(message_id))
            .filter(
                message_recipient::Column::MessageId
                    .in_subquery(active_message_id_subquery(tenant_id, now)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 将当前用户的全部未读消息标记为已读。
    pub async fn mark_all_read(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64> {
        let result = message_recipient::Entity::update_many()
            .col_expr(
                message_recipient::Column::ReadAt,
                sea_orm::sea_query::Expr::cust_with_values("COALESCE(`read_at`, ?)", [now]),
            )
            .col_expr(
                message_recipient::Column::AckedAt,
                sea_orm::sea_query::Expr::cust_with_values("COALESCE(`acked_at`, ?)", [now]),
            )
            .filter(message_recipient::Column::TenantId.eq(tenant_id))
            .filter(message_recipient::Column::UserId.eq(user_id))
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

    /// 删除一批到期消息；关联的受众和收件箱记录由外键级联删除。
    pub async fn delete_expired_batch(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
        batch_size: u64,
    ) -> AppResult<u64> {
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

    async fn resolve_recipients(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        audiences: &BTreeSet<MessageAudienceSelector>,
    ) -> AppResult<BTreeSet<i64>> {
        let mut recipients = BTreeSet::new();
        for selector in audiences {
            match selector.kind {
                MessageAudienceKind::Tenant => {
                    if selector.target_id != 0 {
                        return Err(AppError::Validation("租户受众的 target_id 必须为 0".into()));
                    }
                    let users = enabled_users(transaction, tenant_id, None).await?;
                    recipients.extend(users);
                }
                MessageAudienceKind::Role => {
                    let role_exists = role::Entity::find_by_id(selector.target_id)
                        .filter(role::Column::TenantId.eq(tenant_id))
                        .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
                        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                        .one(transaction)
                        .await
                        .map_err(database_error)?
                        .is_some();
                    if !role_exists {
                        return Err(AppError::Validation("消息目标角色不存在或不可用".into()));
                    }
                    let ids = user_role::Entity::find()
                        .filter(user_role::Column::TenantId.eq(tenant_id))
                        .filter(user_role::Column::RoleId.eq(selector.target_id))
                        .all(transaction)
                        .await
                        .map_err(database_error)?
                        .into_iter()
                        .map(|binding| binding.user_id)
                        .collect::<Vec<_>>();
                    recipients.extend(enabled_users(transaction, tenant_id, Some(ids)).await?);
                }
                MessageAudienceKind::User => {
                    let users =
                        enabled_users(transaction, tenant_id, Some(vec![selector.target_id]))
                            .await?;
                    if users.is_empty() {
                        return Err(AppError::Validation("消息目标用户不存在或不可用".into()));
                    }
                    recipients.extend(users);
                }
            }
        }
        Ok(recipients)
    }
}

async fn enabled_users(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    ids: Option<Vec<i64>>,
) -> AppResult<Vec<i64>> {
    let mut query = user::Entity::find()
        .filter(user::Column::TenantId.eq(tenant_id))
        .filter(user::Column::Status.eq(user::Model::STATUS_NORMAL))
        .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL));
    if let Some(ids) = ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        query = query.filter(user::Column::Id.is_in(ids));
    }
    query
        .all(transaction)
        .await
        .map_err(database_error)
        .map(|users| users.into_iter().map(|model| model.id).collect())
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

fn validate_publish_command(command: &PublishMessageCommand) -> AppResult<()> {
    if command.tenant_id.trim().is_empty()
        || command.topic.trim().is_empty()
        || command.topic.len() > 64
    {
        return Err(AppError::Validation(
            "消息主题、标题或正文不符合长度要求".into(),
        ));
    }
    let literal = matches!(
        (
            &command.title_text,
            &command.body_text,
            &command.title_key,
            &command.body_key
        ),
        (Some(_), Some(_), None, None)
    );
    let keyed = matches!(
        (
            &command.title_text,
            &command.body_text,
            &command.title_key,
            &command.body_key
        ),
        (None, None, Some(_), Some(_))
    );
    if !literal && !keyed {
        return Err(AppError::Validation(
            "消息文本和本地化键必须二选一，且标题与正文需成对提供".into(),
        ));
    }
    if literal
        && (command
            .title_text
            .as_deref()
            .is_none_or(|value| value.trim().is_empty() || value.chars().count() > 200)
            || command
                .body_text
                .as_deref()
                .is_none_or(|value| value.trim().is_empty() || value.chars().count() > 10_000)
            || command.args_json.is_some())
    {
        return Err(AppError::Validation(
            "消息标题或正文不符合长度要求，纯文本消息不能携带本地化参数".into(),
        ));
    }
    if keyed
        && (command
            .title_key
            .as_deref()
            .is_none_or(|value| value.trim().is_empty() || value.len() > 128)
            || command
                .body_key
                .as_deref()
                .is_none_or(|value| value.trim().is_empty() || value.len() > 128))
    {
        return Err(AppError::Validation("消息本地化键不符合长度要求".into()));
    }
    if let Some(args) = &command.args_json
        && serde_json::to_vec(args)
            .map_err(|error| AppError::Validation(format!("消息本地化参数无法序列化: {error}")))?
            .len()
            > 8 * 1024
    {
        return Err(AppError::Validation("消息本地化参数不能超过 8 KiB".into()));
    }
    if keyed
        && command.args_json.as_ref().is_some_and(|args| {
            !args.as_object().is_some_and(|entries| {
                entries
                    .iter()
                    .all(|(key, value)| !key.trim().is_empty() && value.is_string())
            })
        })
    {
        return Err(AppError::Validation(
            "消息本地化参数必须是键和值均为字符串的对象".into(),
        ));
    }
    if !matches!(
        command.severity.as_str(),
        message::Model::SEVERITY_INFO
            | message::Model::SEVERITY_SUCCESS
            | message::Model::SEVERITY_WARNING
            | message::Model::SEVERITY_ERROR
    ) {
        return Err(AppError::Validation("消息级别无效".into()));
    }
    if command.audiences.is_empty() || command.audiences.len() > 500 {
        return Err(AppError::Validation(
            "消息受众数量必须在 1 到 500 之间".into(),
        ));
    }
    if let Some(payload) = &command.payload_json
        && serde_json::to_vec(payload)
            .map_err(|error| AppError::Validation(format!("消息载荷无法序列化: {error}")))?
            .len()
            > 16 * 1024
    {
        return Err(AppError::Validation("消息载荷不能超过 16 KiB".into()));
    }
    if command.expires_at <= command.published_at {
        return Err(AppError::Validation("消息过期时间必须晚于发布时间".into()));
    }
    Ok(())
}

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("duplicate") || text.contains("1062")
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
