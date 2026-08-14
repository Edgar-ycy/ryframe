use super::*;

pub(super) struct NewExecution<'a> {
    pub(super) fire_key: &'a str,
    pub(super) trigger_kind: &'a str,
    pub(super) scheduled_for: DateTime<Utc>,
    pub(super) outcome: &'a str,
    pub(super) detail: Option<String>,
    pub(super) created_at: DateTime<Utc>,
}

pub(super) async fn insert_execution(
    transaction: &DatabaseTransaction,
    schedule: &job_schedule::Model,
    execution: NewExecution<'_>,
) -> AppResult<job_schedule_execution::Model> {
    job_schedule_execution::ActiveModel {
        id: Set(snowflake::try_next_snowflake_id()?),
        tenant_id: Set(schedule.tenant_id.clone()),
        schedule_id: Set(schedule.id),
        schedule_name_snapshot: Set(schedule.name.clone()),
        handler_key_snapshot: Set(schedule.handler_key.clone()),
        fire_key: Set(execution.fire_key.to_owned()),
        trigger_kind: Set(execution.trigger_kind.to_owned()),
        scheduled_for: Set(execution.scheduled_for),
        outcome: Set(execution.outcome.to_owned()),
        background_job_id: Set(None),
        detail: Set(execution.detail.map(|value| truncate_detail(&value))),
        created_at: Set(execution.created_at),
    }
    .insert(transaction)
    .await
    .map_err(database_error)
}

pub(super) async fn attach_background_job(
    transaction: &DatabaseTransaction,
    execution: job_schedule_execution::Model,
    background_job_id: i64,
) -> AppResult<job_schedule_execution::Model> {
    let mut active: job_schedule_execution::ActiveModel = execution.into();
    active.background_job_id = Set(Some(background_job_id));
    active.update(transaction).await.map_err(database_error)
}

pub(super) async fn advance_schedule(
    transaction: &DatabaseTransaction,
    schedule: job_schedule::Model,
    next_run_at: DateTime<Utc>,
    last_run_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let mut active: job_schedule::ActiveModel = schedule.into();
    active.next_run_at = Set(Some(next_run_at));
    active.last_run_at = Set(Some(last_run_at));
    active.updated_at = Set(now);
    active.update(transaction).await.map_err(database_error)?;
    Ok(())
}

pub(super) async fn quarantine_invalid_schedule(
    transaction: &DatabaseTransaction,
    schedule: job_schedule::Model,
    now: DateTime<Utc>,
    detail: &str,
) -> AppResult<()> {
    let scheduled_for = schedule.next_run_at.unwrap_or(now);
    let fire_key = format!(
        "invalid:{}:{}",
        schedule.version,
        automatic_fire_key(scheduled_for)
    );
    insert_execution(
        transaction,
        &schedule,
        NewExecution {
            fire_key: &fire_key,
            trigger_kind: job_schedule_execution::Model::TRIGGER_SCHEDULED,
            scheduled_for,
            outcome: job_schedule_execution::Model::OUTCOME_INVALID_CONFIGURATION,
            detail: Some(detail.to_owned()),
            created_at: now,
        },
    )
    .await?;

    let next_version = schedule.version.saturating_add(1);
    let mut active: job_schedule::ActiveModel = schedule.into();
    active.enabled = Set(false);
    active.next_run_at = Set(None);
    active.version = Set(next_version);
    active.updated_at = Set(now);
    active.update(transaction).await.map_err(database_error)?;
    Ok(())
}

pub(super) async fn lock_tenant(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
) -> AppResult<()> {
    let row = transaction
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::MySql,
            "SELECT tenant_id FROM sys_tenant WHERE tenant_id = ? FOR UPDATE",
            [tenant_id.into()],
        ))
        .await
        .map_err(database_error)?;
    if row.is_none() {
        return Err(AppError::NotFound("当前租户不存在".into()));
    }
    Ok(())
}

pub(super) fn execution_into_vo(
    execution: job_schedule_execution::Model,
    background_job_status: Option<String>,
) -> JobScheduleExecutionVo {
    JobScheduleExecutionVo {
        id: execution.id.to_string(),
        schedule_id: execution.schedule_id.to_string(),
        schedule_name: execution.schedule_name_snapshot,
        handler_key: execution.handler_key_snapshot,
        trigger_kind: execution.trigger_kind,
        scheduled_for: execution.scheduled_for,
        outcome: execution.outcome,
        background_job_id: execution.background_job_id.map(|id| id.to_string()),
        background_job_status,
        detail: execution.detail,
        created_at: execution.created_at,
    }
}

pub(super) fn normalize_required(value: &str, max_bytes: usize, label: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes {
        return Err(AppError::Validation(format!(
            "{label}必须为 1 到 {max_bytes} 字节"
        )));
    }
    Ok(value.to_owned())
}

pub(super) fn normalize_optional(
    value: &Option<String>,
    max_bytes: usize,
    label: &str,
) -> AppResult<Option<String>> {
    value
        .as_deref()
        .map(|value| normalize_required(value, max_bytes, label))
        .transpose()
}

pub(super) fn normalize_trigger_kind(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &[
            job_schedule_execution::Model::TRIGGER_SCHEDULED,
            job_schedule_execution::Model::TRIGGER_MISFIRE,
            job_schedule_execution::Model::TRIGGER_MANUAL,
        ],
        "触发类型",
    )
}

pub(super) fn normalize_outcome(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &[
            job_schedule_execution::Model::OUTCOME_ENQUEUED,
            job_schedule_execution::Model::OUTCOME_SKIPPED_MISFIRE,
            job_schedule_execution::Model::OUTCOME_SKIPPED_CONCURRENCY,
            job_schedule_execution::Model::OUTCOME_TARGET_UNAVAILABLE,
            job_schedule_execution::Model::OUTCOME_INVALID_CONFIGURATION,
        ],
        "执行结果",
    )
}

pub(super) fn normalize_background_status(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &["pending", "running", "succeeded", "dead"],
        "后台任务状态",
    )
}

pub(super) fn normalize_enum_filter(
    value: Option<String>,
    allowed: &[&str],
    label: &str,
) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if !allowed.contains(&value) {
        return Err(AppError::Validation(format!("{label}无效")));
    }
    Ok(Some(value.to_owned()))
}

pub(super) fn normalize_idempotency_key(value: &str) -> AppResult<&str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 1 到 255 字节".into(),
        ));
    }
    Ok(value)
}

pub(super) fn manual_fire_key(idempotency_key: &str) -> String {
    format!(
        "manual:{}",
        hex::encode(Sha256::digest(idempotency_key.as_bytes()))
    )
}

pub(super) fn automatic_fire_key(scheduled_for: DateTime<Utc>) -> String {
    format!("auto:{}", scheduled_for.timestamp_micros())
}

pub(super) fn validate_id(id: i64) -> AppResult<()> {
    if id <= 0 {
        return Err(AppError::Validation("定时任务 ID 必须是正整数".into()));
    }
    Ok(())
}

pub(super) fn validate_version(version: i64) -> AppResult<()> {
    if version <= 0 {
        return Err(AppError::Validation("version 必须是正整数".into()));
    }
    Ok(())
}

pub(super) fn truncate_detail(value: &str) -> String {
    const MAX_BYTES: usize = 2_000;
    if value.len() <= MAX_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

pub(super) async fn rollback_with<T>(
    transaction: DatabaseTransaction,
    error: AppError,
) -> AppResult<T> {
    if let Err(rollback_error) = transaction.rollback().await {
        tracing::warn!(%rollback_error, "回滚调度事务失败");
    }
    Err(error)
}
