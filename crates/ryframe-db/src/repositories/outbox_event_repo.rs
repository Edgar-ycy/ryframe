use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
    sea_query::{Expr, LockBehavior, LockType},
};
use serde_json::Value;

use crate::entities::outbox_event;

const EXPIRED_LEASE_DEAD_ERROR: &str = "Outbox 事件租约已过期，投递结果未知";

/// 写入事务 Outbox 的输入。
#[derive(Clone, Debug)]
pub struct RecordOutboxEvent {
    pub tenant_id: Option<String>,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i32,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// Outbox 投递失败后的状态转换结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboxFailureDisposition {
    Retried { available_at: DateTime<Utc> },
    Dead,
    LeaseLost,
}

/// 仅适用于 MySQL 的事务 Outbox 仓储。
pub struct OutboxEventRepository;

impl OutboxEventRepository {
    /// 获取数据库 UTC 时间，避免多个 Worker 依赖不同机器时钟。
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
    where
        C: ConnectionTrait,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("数据库时钟查询未返回记录".into()))?;
        let now: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
        Ok(DateTime::from_naive_utc_and_offset(now, Utc))
    }

    /// 在调用方事务中记录事件，确保业务写入与异步投递意图原子提交。
    pub async fn record_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        event: RecordOutboxEvent,
        now: DateTime<Utc>,
    ) -> AppResult<outbox_event::Model> {
        validate_event(&event)?;
        let dedupe_identity = event
            .dedupe_key
            .as_ref()
            .map(|dedupe_key| (event.event_type.clone(), dedupe_key.clone()));
        if let Some((event_type, dedupe_key)) = dedupe_identity.as_ref()
            && let Some(existing) = self
                .find_by_dedupe_key_on(transaction, event_type, dedupe_key)
                .await?
        {
            return Ok(existing);
        }

        let active = outbox_event::ActiveModel {
            id: Set(snowflake::try_next_snowflake_id()?),
            tenant_id: Set(event.tenant_id),
            event_type: Set(event.event_type),
            aggregate_type: Set(event.aggregate_type),
            aggregate_id: Set(event.aggregate_id),
            payload: Set(event.payload),
            status: Set(outbox_event::Model::STATUS_PENDING.to_owned()),
            available_at: Set(event.available_at),
            attempts: Set(0),
            max_attempts: Set(event.max_attempts),
            lease_owner: Set(None),
            lease_until: Set(None),
            dedupe_key: Set(event.dedupe_key),
            traceparent: Set(event.traceparent),
            tracestate: Set(event.tracestate),
            last_error: Set(None),
            published_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        match active.insert(transaction).await {
            Ok(event) => Ok(event),
            Err(error) if is_duplicate_key_error(&error) => {
                let (event_type, dedupe_key) = dedupe_identity
                    .ok_or_else(|| AppError::Database("Outbox 重复写入未提供幂等键".into()))?;
                self.find_by_dedupe_key_on(transaction, &event_type, &dedupe_key)
                    .await?
                    .ok_or_else(|| AppError::Database("Outbox 重复键已报告但记录不可读".into()))
            }
            Err(error) => Err(database_error(error)),
        }
    }

    /// 使用行锁领取一条到期可投递事件。领取即消耗一次尝试预算。
    pub async fn claim_next(
        &self,
        db: &DatabaseConnection,
        worker_id: &str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> AppResult<Option<outbox_event::Model>> {
        validate_lease(worker_id, lease_duration)?;
        let transaction = db.begin().await.map_err(database_error)?;
        let Some(event) = Self::claimable_query(now)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let attempts = event
            .attempts
            .checked_add(1)
            .ok_or_else(|| AppError::Database("Outbox 事件尝试次数溢出".into()))?;
        let mut active: outbox_event::ActiveModel = event.into();
        active.status = Set(outbox_event::Model::STATUS_RUNNING.to_owned());
        active.attempts = Set(attempts);
        active.lease_owner = Set(Some(worker_id.to_owned()));
        active.lease_until = Set(Some(now + lease_duration));
        active.updated_at = Set(now);
        let claimed = active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(claimed))
    }

    /// 在包含下游副作用的同一事务中确认事件已投递。
    pub async fn mark_published_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        event_id: i64,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = outbox_event::Entity::update_many()
            .col_expr(
                outbox_event::Column::Status,
                Expr::value(outbox_event::Model::STATUS_PUBLISHED),
            )
            .col_expr(
                outbox_event::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                outbox_event::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(outbox_event::Column::PublishedAt, Expr::value(now))
            .col_expr(outbox_event::Column::UpdatedAt, Expr::value(now))
            .filter(outbox_event::Column::Id.eq(event_id))
            .filter(outbox_event::Column::Status.eq(outbox_event::Model::STATUS_RUNNING))
            .filter(outbox_event::Column::LeaseOwner.eq(worker_id))
            .filter(outbox_event::Column::LeaseUntil.gt(now))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 将处理失败的事件重新排期，或在尝试预算耗尽后进入死信状态。
    pub async fn fail(
        &self,
        db: &DatabaseConnection,
        event_id: i64,
        worker_id: &str,
        retry_at: DateTime<Utc>,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<OutboxFailureDisposition> {
        let transaction = db.begin().await.map_err(database_error)?;
        let Some(event) = Self::owned_running_query(event_id, worker_id)
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            rollback_quietly(transaction).await;
            return Ok(OutboxFailureDisposition::LeaseLost);
        };
        if event
            .lease_until
            .is_none_or(|lease_until| lease_until <= now)
        {
            rollback_quietly(transaction).await;
            return Ok(OutboxFailureDisposition::LeaseLost);
        }
        let dead = event.attempts >= event.max_attempts;
        let mut active: outbox_event::ActiveModel = event.into();
        active.status = Set(if dead {
            outbox_event::Model::STATUS_DEAD
        } else {
            outbox_event::Model::STATUS_PENDING
        }
        .to_owned());
        active.available_at = Set(if dead { now } else { retry_at });
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.last_error = Set(Some(truncate_error(error_message)));
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(if dead {
            OutboxFailureDisposition::Dead
        } else {
            OutboxFailureDisposition::Retried {
                available_at: retry_at,
            }
        })
    }

    fn claimable_query(now: DateTime<Utc>) -> sea_orm::Select<outbox_event::Entity> {
        outbox_event::Entity::find()
            .filter(outbox_event::Column::Status.eq(outbox_event::Model::STATUS_PENDING))
            .filter(outbox_event::Column::AvailableAt.lte(now))
            .filter(
                Expr::col(outbox_event::Column::Attempts)
                    .lt(Expr::col(outbox_event::Column::MaxAttempts)),
            )
            .order_by_asc(outbox_event::Column::AvailableAt)
            .order_by_asc(outbox_event::Column::Id)
    }

    fn owned_running_query(
        event_id: i64,
        worker_id: &str,
    ) -> sea_orm::Select<outbox_event::Entity> {
        outbox_event::Entity::find_by_id(event_id)
            .filter(outbox_event::Column::Status.eq(outbox_event::Model::STATUS_RUNNING))
            .filter(outbox_event::Column::LeaseOwner.eq(worker_id))
    }

    async fn find_by_dedupe_key_on<C>(
        &self,
        db: &C,
        event_type: &str,
        dedupe_key: &str,
    ) -> AppResult<Option<outbox_event::Model>>
    where
        C: ConnectionTrait,
    {
        outbox_event::Entity::find()
            .filter(outbox_event::Column::EventType.eq(event_type))
            .filter(outbox_event::Column::DedupeKey.eq(dedupe_key))
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 回收已过期的 Worker 租约。
    ///
    /// 该维护操作由单独的恢复循环调用，避免与并发领取操作位于同一事务中，
    /// 从而保持统一的锁顺序并降低 MySQL 死锁风险。
    pub async fn recover_expired_leases(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        self.recover_expired_leases_on(db, now).await
    }

    async fn recover_expired_leases_on<C>(&self, db: &C, now: DateTime<Utc>) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        outbox_event::Entity::update_many()
            .col_expr(
                outbox_event::Column::Status,
                Expr::value(outbox_event::Model::STATUS_DEAD),
            )
            .col_expr(
                outbox_event::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                outbox_event::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(
                outbox_event::Column::LastError,
                Expr::value(Some(EXPIRED_LEASE_DEAD_ERROR.to_owned())),
            )
            .col_expr(outbox_event::Column::UpdatedAt, Expr::value(now))
            .filter(outbox_event::Column::Status.eq(outbox_event::Model::STATUS_RUNNING))
            .filter(outbox_event::Column::LeaseUntil.lte(now))
            .filter(
                Expr::col(outbox_event::Column::Attempts)
                    .gte(Expr::col(outbox_event::Column::MaxAttempts)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        outbox_event::Entity::update_many()
            .col_expr(
                outbox_event::Column::Status,
                Expr::value(outbox_event::Model::STATUS_PENDING),
            )
            .col_expr(
                outbox_event::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                outbox_event::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(outbox_event::Column::AvailableAt, Expr::value(now))
            .col_expr(outbox_event::Column::UpdatedAt, Expr::value(now))
            .filter(outbox_event::Column::Status.eq(outbox_event::Model::STATUS_RUNNING))
            .filter(outbox_event::Column::LeaseUntil.lte(now))
            .filter(
                Expr::col(outbox_event::Column::Attempts)
                    .lt(Expr::col(outbox_event::Column::MaxAttempts)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(())
    }
}

fn validate_event(event: &RecordOutboxEvent) -> AppResult<()> {
    for (name, value, maximum) in [
        ("event_type", event.event_type.as_str(), 96),
        ("aggregate_type", event.aggregate_type.as_str(), 64),
        ("aggregate_id", event.aggregate_id.as_str(), 128),
    ] {
        if value.is_empty() || value.len() > maximum {
            return Err(AppError::Validation(format!(
                "{name} 长度必须介于 1 和 {maximum} 之间"
            )));
        }
    }
    if !(1..=20).contains(&event.max_attempts) {
        return Err(AppError::Validation(
            "max_attempts 必须介于 1 和 20 之间".into(),
        ));
    }
    if event
        .dedupe_key
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > 191)
    {
        return Err(AppError::Validation(
            "dedupe_key 长度必须介于 1 和 191 之间".into(),
        ));
    }
    if event
        .traceparent
        .as_deref()
        .is_some_and(|value| value.len() > 255)
    {
        return Err(AppError::Validation(
            "Outbox traceparent 不能超过 255 字节".into(),
        ));
    }
    if event
        .tracestate
        .as_deref()
        .is_some_and(|value| value.len() > 512)
    {
        return Err(AppError::Validation(
            "Outbox tracestate 不能超过 512 字节".into(),
        ));
    }
    Ok(())
}

fn validate_lease(worker_id: &str, lease_duration: Duration) -> AppResult<()> {
    if worker_id.is_empty() || worker_id.len() > 128 {
        return Err(AppError::Validation(
            "Outbox Worker 标识长度必须介于 1 和 128 之间".into(),
        ));
    }
    if lease_duration <= Duration::zero() {
        return Err(AppError::Validation("Outbox 租约必须大于零".into()));
    }
    Ok(())
}

async fn rollback_quietly(transaction: DatabaseTransaction) {
    let _ = transaction.rollback().await;
}

fn truncate_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 4_000;
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &error[..end])
}

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let message = error.to_string();
    message.contains("Duplicate entry") || message.contains("1062")
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
