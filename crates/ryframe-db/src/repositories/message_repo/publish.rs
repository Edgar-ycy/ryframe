use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::sea_query::{Expr, Order, Query};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, ConnectionTrait,
    DatabaseTransaction, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect, QueryTrait,
};

use crate::entities::{message, message_audience, message_recipient, role, user, user_role};

use super::{
    MessageAudienceKind, MessageAudienceSelector, MessageRepository, PublishMessageCommand,
    PublishedMessage, database_error,
};

impl MessageRepository {
    /// 在调用方事务中插入消息和受众，并通过数据库侧 `INSERT … SELECT` 固化收件箱快照。
    pub async fn publish_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: PublishMessageCommand,
        max_recipients: u64,
    ) -> AppResult<PublishedMessage> {
        validate_publish_command(&command)?;
        if max_recipients == 0 {
            return Err(AppError::Config(
                "messaging.max_recipients_per_message 必须大于 0".into(),
            ));
        }
        let audiences = command.audiences.iter().cloned().collect::<BTreeSet<_>>();
        let model = message::Model {
            id: crate::next_id()?,
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
                if recipient_count as u64 > max_recipients {
                    return Err(AppError::Validation(format!(
                        "消息收件人数不能超过 {max_recipients}"
                    )));
                }
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

        let selector_sets =
            validate_audience_targets(transaction, &inserted.tenant_id, &audiences).await?;
        let recipient_count =
            insert_recipient_snapshot(transaction, &inserted, &selector_sets, max_recipients)
                .await?;
        if recipient_count == 0 {
            return Err(AppError::Validation(
                "消息受众中没有可投递的启用用户".into(),
            ));
        }
        if recipient_count > max_recipients {
            return Err(AppError::Validation(format!(
                "消息收件人数不能超过 {max_recipients}"
            )));
        }

        Ok(PublishedMessage {
            message: inserted,
            recipient_count: usize::try_from(recipient_count)
                .map_err(|_| AppError::Internal("消息收件人数超出平台整数范围".into()))?,
            inserted: true,
        })
    }
}

struct AudienceSelectorSets {
    includes_tenant: bool,
    role_ids: Vec<i64>,
    user_ids: Vec<i64>,
}

async fn validate_audience_targets(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    audiences: &BTreeSet<MessageAudienceSelector>,
) -> AppResult<AudienceSelectorSets> {
    let mut includes_tenant = false;
    let mut role_ids = Vec::new();
    let mut user_ids = Vec::new();
    for selector in audiences {
        match selector.kind {
            MessageAudienceKind::Tenant => {
                if selector.target_id != 0 {
                    return Err(AppError::Validation("租户受众的 target_id 必须为 0".into()));
                }
                includes_tenant = true;
            }
            MessageAudienceKind::Role => role_ids.push(selector.target_id),
            MessageAudienceKind::User => user_ids.push(selector.target_id),
        }
    }

    if !role_ids.is_empty() {
        let valid_role_count = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::Id.is_in(role_ids.clone()))
            .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .count(transaction)
            .await
            .map_err(database_error)?;
        if valid_role_count != role_ids.len() as u64 {
            return Err(AppError::Validation("消息目标角色不存在或不可用".into()));
        }
    }
    if !user_ids.is_empty() {
        let valid_user_count = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Id.is_in(user_ids.clone()))
            .filter(user::Column::Status.eq(user::Model::STATUS_NORMAL))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .count(transaction)
            .await
            .map_err(database_error)?;
        if valid_user_count != user_ids.len() as u64 {
            return Err(AppError::Validation("消息目标用户不存在或不可用".into()));
        }
    }

    Ok(AudienceSelectorSets {
        includes_tenant,
        role_ids,
        user_ids,
    })
}

async fn insert_recipient_snapshot(
    transaction: &DatabaseTransaction,
    message: &message::Model,
    selectors: &AudienceSelectorSets,
    max_recipients: u64,
) -> AppResult<u64> {
    let mut audience_condition = Condition::any();
    if selectors.includes_tenant {
        audience_condition = audience_condition.add(Expr::value(true));
    }
    if !selectors.user_ids.is_empty() {
        audience_condition =
            audience_condition.add(user::Column::Id.is_in(selectors.user_ids.clone()));
    }
    if !selectors.role_ids.is_empty() {
        let role_user_ids = user_role::Entity::find()
            .select_only()
            .column(user_role::Column::UserId)
            .filter(user_role::Column::TenantId.eq(&message.tenant_id))
            .filter(user_role::Column::RoleId.is_in(selectors.role_ids.clone()))
            .into_query();
        audience_condition = audience_condition.add(user::Column::Id.in_subquery(role_user_ids));
    }

    let mut recipient_select = Query::select();
    recipient_select
        .expr(Expr::value(message.id))
        .column(user::Column::Id)
        .expr(Expr::value(message.tenant_id.clone()))
        .expr(Expr::value(message.published_at))
        .expr(Expr::value(Option::<DateTime<Utc>>::None))
        .expr(Expr::value(Option::<DateTime<Utc>>::None))
        .expr(Expr::value(Option::<DateTime<Utc>>::None))
        .expr(Expr::value(Option::<DateTime<Utc>>::None))
        .from(user::Entity)
        .and_where(user::Column::TenantId.eq(&message.tenant_id))
        .and_where(user::Column::Status.eq(user::Model::STATUS_NORMAL))
        .and_where(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
        .cond_where(audience_condition)
        .order_by(user::Column::Id, Order::Asc)
        .limit(max_recipients.saturating_add(1));

    let insert = Query::insert()
        .into_table(message_recipient::Entity)
        .columns([
            message_recipient::Column::MessageId,
            message_recipient::Column::UserId,
            message_recipient::Column::TenantId,
            message_recipient::Column::CreatedAt,
            message_recipient::Column::EnqueuedAt,
            message_recipient::Column::AckedAt,
            message_recipient::Column::ReadAt,
            message_recipient::Column::DeletedAt,
        ])
        .select_from(recipient_select)
        .map_err(|error| AppError::Internal(format!("构造消息收件人快照失败: {error}")))?
        .to_owned();
    transaction
        .execute(&insert)
        .await
        .map_err(database_error)
        .map(|result| result.rows_affected())
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
