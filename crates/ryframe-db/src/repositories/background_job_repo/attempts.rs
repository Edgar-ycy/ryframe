use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    sea_query::Expr,
};

use crate::{
    DbResultExt,
    entities::{background_job, background_job_attempt},
};

pub(super) fn next_sequence(sequence: i64) -> AppResult<i64> {
    sequence
        .checked_add(1)
        .filter(|value| *value > 0)
        .ok_or_else(|| AppError::Database("任务领取序号溢出或无效".into()))
}

pub(super) async fn start<C: ConnectionTrait>(
    db: &C,
    job: &background_job::Model,
    sequence: i64,
    now: DateTime<Utc>,
) -> AppResult<()> {
    if job.status != background_job::Model::STATUS_PENDING
        || job.lease_owner.is_some()
        || job.lease_until.is_some()
        || sequence != next_sequence(job.claim_sequence)?
        || now < job.available_at
    {
        return Err(AppError::Database("任务领取状态、时间或序号无效".into()));
    }
    background_job_attempt::ActiveModel {
        job_id: Set(job.id),
        sequence: Set(sequence),
        available_at: Set(job.available_at),
        started_at: Set(now),
        finished_at: Set(None),
        closed_at: Set(None),
        outcome: Set("running".into()),
    }
    .insert(db)
    .await
    .db()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Outcome {
    Succeeded,
    Failed,
    Dead,
    Deferred,
    LeaseExpired,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Dead => "dead",
            Self::Deferred => "deferred",
            Self::LeaseExpired => "lease_expired",
        }
    }

    fn finished_at(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        (!matches!(self, Self::LeaseExpired)).then_some(now)
    }
}

/// 仅持有任务行锁及当前领取序号的事务可以闭合对应尝试。
/// 租约过期仅记录回收时刻，不虚构 Worker 实际完成时间。
pub(super) async fn close<C: ConnectionTrait>(
    db: &C,
    job: &background_job::Model,
    outcome: Outcome,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let changed = background_job_attempt::Entity::update_many()
        .col_expr(
            background_job_attempt::Column::Outcome,
            Expr::value(outcome.as_str()),
        )
        .col_expr(
            background_job_attempt::Column::FinishedAt,
            Expr::value(outcome.finished_at(now)),
        )
        .col_expr(background_job_attempt::Column::ClosedAt, Expr::value(now))
        .filter(background_job_attempt::Column::JobId.eq(job.id))
        .filter(background_job_attempt::Column::Sequence.eq(job.claim_sequence))
        .filter(background_job_attempt::Column::Outcome.eq("running"))
        .filter(background_job_attempt::Column::StartedAt.lte(now))
        .filter(background_job_attempt::Column::ClosedAt.is_null())
        .exec(db)
        .await
        .db()?;
    if changed.rows_affected != 1 {
        return Err(AppError::Database(
            "任务尝试记录缺失、已闭合或时间逆序".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_sequence_never_reuses_retry_budget_and_rejects_overflow() {
        assert_eq!(next_sequence(0).unwrap(), 1);
        assert_eq!(next_sequence(100).unwrap(), 101);
        assert!(next_sequence(-1).is_err());
        assert!(next_sequence(i64::MAX).is_err());
    }

    #[test]
    fn expired_lease_closes_history_without_inventing_worker_finish() {
        let now = DateTime::from_timestamp(1_800_000_000, 123_000).unwrap();
        assert_eq!(Outcome::LeaseExpired.finished_at(now), None);
        assert_eq!(Outcome::LeaseExpired.as_str(), "lease_expired");
        for outcome in [
            Outcome::Succeeded,
            Outcome::Failed,
            Outcome::Dead,
            Outcome::Deferred,
        ] {
            assert_eq!(outcome.finished_at(now), Some(now));
        }
    }
}
