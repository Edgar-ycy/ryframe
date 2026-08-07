use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter, Statement,
};

use crate::entities::background_job;

use super::{
    BackgroundJobRepository, EnqueueBackgroundJob, EnqueueBackgroundJobResult, database_error,
};

impl BackgroundJobRepository {
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
            .ok_or_else(|| AppError::Database("database clock query returned no row".into()))?;
        let now: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
        Ok(DateTime::from_naive_utc_and_offset(now, Utc))
    }

    pub async fn enqueue(
        &self,
        db: &DatabaseConnection,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(db, command, now).await
    }

    /// 在调用方事务中入队，使业务数据与任务记录原子提交。
    pub async fn enqueue_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(transaction, command, now).await
    }

    async fn enqueue_on<C>(
        &self,
        db: &C,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult>
    where
        C: ConnectionTrait,
    {
        validate_enqueue_command(&command)?;
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
            id: Set(snowflake::try_next_snowflake_id()?),
            tenant_id: Set(command.tenant_id),
            job_type: Set(command.job_type),
            payload: Set(command.payload),
            status: Set(background_job::Model::STATUS_PENDING.to_owned()),
            priority: Set(command.priority),
            available_at: Set(command.available_at),
            attempts: Set(0),
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
            Err(error) => Err(database_error(error)),
        }
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        background_job::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 查询当前租户可见的单个任务。
    pub async fn find_by_id_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        background_job::Entity::find_by_id(id)
            .filter(background_job::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
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
            .map_err(database_error)
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

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("duplicate") || text.contains("1062")
}
