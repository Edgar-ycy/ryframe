use crate::DbResultExt;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, ConnectionTrait,
    DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter,
};
use time::{OffsetDateTime, UtcOffset};

use crate::entities::background_job;

use super::{BackgroundJobRepository, EnqueueBackgroundJob, EnqueueBackgroundJobResult};

impl BackgroundJobRepository {
    pub async fn enqueue(
        &self,
        db: &DatabaseConnection,
        command: EnqueueBackgroundJob,
        now: OffsetDateTime,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(db, command, now).await
    }

    /// 在调用方事务中入队，使业务数据与任务记录原子提交。
    pub async fn enqueue_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: EnqueueBackgroundJob,
        now: OffsetDateTime,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(transaction, command, now).await
    }

    async fn enqueue_on<C>(
        &self,
        db: &C,
        mut command: EnqueueBackgroundJob,
        now: OffsetDateTime,
    ) -> AppResult<EnqueueBackgroundJobResult>
    where
        C: ConnectionTrait,
    {
        validate_enqueue_command(&command)?;
        let now = normalize_enqueue_times(&mut command, now)?;
        let dedupe_identity = command
            .dedupe_key
            .as_ref()
            .map(|dedupe_key| (command.job_type.clone(), dedupe_key.clone()));

        if let Some((job_type, dedupe_key)) = dedupe_identity.as_ref()
            && let Some(existing) = self.find_by_dedupe_key_on(db, job_type, dedupe_key).await?
        {
            return Ok(EnqueueBackgroundJobResult {
                job: existing,
                inserted: false,
            });
        }

        let active = background_job::ActiveModel {
            id: Set(crate::next_id()?),
            tenant_id: Set(command.tenant_id),
            schedule_id: Set(command.schedule_id),
            scheduled_for: Set(command.scheduled_for),
            max_runtime_seconds: Set(command.max_runtime_seconds),
            job_type: Set(command.job_type),
            payload: Set(command.payload),
            status: Set(background_job::Model::STATUS_PENDING.to_owned()),
            priority: Set(command.priority),
            available_at: Set(command.available_at),
            attempts: Set(0),
            claim_sequence: Set(0),
            max_attempts: Set(command.max_attempts),
            lease_owner: Set(None),
            lease_until: Set(None),
            dedupe_key: Set(command.dedupe_key),
            traceparent: Set(command.traceparent),
            tracestate: Set(command.tracestate),
            last_error: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
        };

        match active.insert(db).await {
            Ok(job) => Ok(EnqueueBackgroundJobResult {
                job,
                inserted: true,
            }),
            Err(error) if is_duplicate_key_error(&error) => {
                // 并发插入可能发生在预检查之后。MySQL 在报告 1062 前会等待获胜事务，
                // 因此后续查询能够读取到该记录。
                let (job_type, dedupe_key) = dedupe_identity.ok_or_else(|| {
                    AppError::Database(
                        "duplicate background job insert without a dedupe key".into(),
                    )
                })?;
                let existing = self
                    .find_by_dedupe_key_on(db, &job_type, &dedupe_key)
                    .await?
                    .ok_or_else(|| {
                        AppError::Database(
                            "duplicate background job key was reported but no row is readable"
                                .into(),
                        )
                    })?;
                Ok(EnqueueBackgroundJobResult {
                    job: existing,
                    inserted: false,
                })
            }
            Err(error) => Err(error).db(),
        }
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        background_job::Entity::find_by_id(id).one(db).await.db()
    }

    /// 查询当前租户可见的单个任务。
    pub async fn find_by_id_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        include_platform: bool,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        let query = background_job::Entity::find_by_id(id);
        let query = if include_platform {
            query.filter(
                Condition::any()
                    .add(background_job::Column::TenantId.eq(tenant_id))
                    .add(background_job::Column::TenantId.is_null()),
            )
        } else {
            query.filter(background_job::Column::TenantId.eq(tenant_id))
        };
        query.one(db).await.db()
    }

    pub async fn find_by_dedupe_key(
        &self,
        db: &DatabaseConnection,
        job_type: &str,
        dedupe_key: &str,
    ) -> AppResult<Option<background_job::Model>> {
        self.find_by_dedupe_key_on(db, job_type, dedupe_key).await
    }

    async fn find_by_dedupe_key_on<C>(
        &self,
        db: &C,
        job_type: &str,
        dedupe_key: &str,
    ) -> AppResult<Option<background_job::Model>>
    where
        C: ConnectionTrait,
    {
        background_job::Entity::find()
            .filter(background_job::Column::JobType.eq(job_type))
            .filter(background_job::Column::DedupeKey.eq(dedupe_key))
            .one(db)
            .await
            .db()
    }
}

fn validate_enqueue_command(command: &EnqueueBackgroundJob) -> AppResult<()> {
    if command.job_type.trim().is_empty() || command.job_type.len() > 96 {
        return Err(AppError::Validation(
            "background job type must contain 1 to 96 bytes".into(),
        ));
    }
    if command.max_attempts <= 0 {
        return Err(AppError::Validation(
            "background job max_attempts must be greater than zero".into(),
        ));
    }
    if command.max_attempts > 100 {
        return Err(AppError::Validation(
            "background job max_attempts must not exceed 100".into(),
        ));
    }
    if command
        .max_runtime_seconds
        .is_some_and(|seconds| !(1..=86_400).contains(&seconds))
    {
        return Err(AppError::Validation(
            "计划任务最大运行时长必须在 1 到 86400 秒之间".into(),
        ));
    }
    if command
        .dedupe_key
        .as_deref()
        .is_some_and(|key| key.is_empty() || key.len() > 191)
    {
        return Err(AppError::Validation(
            "background job dedupe_key must contain 1 to 191 bytes when supplied".into(),
        ));
    }
    if command
        .traceparent
        .as_deref()
        .is_some_and(|value| value.len() > 255)
    {
        return Err(AppError::Validation(
            "background job traceparent must not exceed 255 bytes".into(),
        ));
    }
    if command
        .tracestate
        .as_deref()
        .is_some_and(|value| value.len() > 512)
    {
        return Err(AppError::Validation(
            "background job tracestate must not exceed 512 bytes".into(),
        ));
    }
    Ok(())
}

fn normalize_enqueue_times(
    command: &mut EnqueueBackgroundJob,
    now: OffsetDateTime,
) -> AppResult<OffsetDateTime> {
    let to_utc = |timestamp: OffsetDateTime, field| {
        timestamp
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(|| AppError::Validation(format!("后台任务 {field} 超出 UTC 表示范围")))
    };
    command.available_at = to_utc(command.available_at, "available_at")?;
    command.scheduled_for = command
        .scheduled_for
        .map(|scheduled_for| to_utc(scheduled_for, "scheduled_for"))
        .transpose()?;
    to_utc(now, "now")
}

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("duplicate") || text.contains("1062")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{
        Date,
        macros::{datetime, offset},
    };

    fn enqueue_command(available_at: OffsetDateTime) -> EnqueueBackgroundJob {
        EnqueueBackgroundJob {
            tenant_id: None,
            schedule_id: None,
            scheduled_for: None,
            max_runtime_seconds: None,
            job_type: "test.queue".into(),
            payload: serde_json::json!({}),
            priority: 0,
            available_at,
            max_attempts: 1,
            dedupe_key: Some("enqueue-time-test".into()),
            traceparent: None,
            tracestate: None,
        }
    }

    #[test]
    fn enqueue_times_normalize_offsets_without_losing_precision() {
        let expected = datetime!(2026-09-01 00:05:00.123456789 UTC);
        let mut command = enqueue_command(expected.to_offset(offset!(-08:00)));
        command.scheduled_for = Some(expected.to_offset(offset!(+05:30)));
        let now =
            normalize_enqueue_times(&mut command, expected.to_offset(offset!(+01:00))).unwrap();
        for value in [command.available_at, command.scheduled_for.unwrap(), now] {
            assert_eq!(value, expected);
            assert_eq!(value.offset(), UtcOffset::UTC);
        }

        command.scheduled_for = None;
        normalize_enqueue_times(&mut command, expected).unwrap();
        assert!(command.scheduled_for.is_none());
    }

    #[tokio::test]
    async fn enqueue_rejects_out_of_range_utc_times_before_dedupe_lookup() {
        let db = DatabaseConnection::default();
        let valid = OffsetDateTime::UNIX_EPOCH;
        let below = Date::MIN.midnight().assume_offset(offset!(+01:00));
        let above = Date::MAX
            .with_hms(23, 59, 59)
            .unwrap()
            .assume_offset(offset!(-01:00));
        for invalid in [below, above] {
            for (field, available_at, scheduled_for, now) in [
                ("available_at", invalid, Some(valid), valid),
                ("scheduled_for", valid, Some(invalid), valid),
                ("now", valid, Some(valid), invalid),
            ] {
                let mut command = enqueue_command(available_at);
                command.scheduled_for = scheduled_for;
                let error = BackgroundJobRepository
                    .enqueue(&db, command, now)
                    .await
                    .unwrap_err();
                assert!(matches!(
                    error,
                    AppError::Validation(message) if message.contains(field)
                ));
            }
        }
    }
}
