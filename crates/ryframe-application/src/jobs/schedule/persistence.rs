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
    transaction: &dyn JobScheduleTransaction,
    schedule: &JobScheduleRecord,
    execution: NewExecution<'_>,
) -> AppResult<JobScheduleExecutionRecord> {
    transaction
        .insert_execution(
            schedule,
            NewJobScheduleExecution {
                id: crate::next_id()?,
                fire_key: execution.fire_key.to_owned(),
                trigger_kind: execution.trigger_kind.to_owned(),
                scheduled_for: execution.scheduled_for,
                outcome: execution.outcome.to_owned(),
                detail: execution.detail.map(|value| truncate_detail(&value)),
                created_at: execution.created_at,
            },
        )
        .await
}

pub(super) async fn attach_background_job(
    transaction: &dyn JobScheduleTransaction,
    execution: JobScheduleExecutionRecord,
    background_job_id: i64,
) -> AppResult<JobScheduleExecutionRecord> {
    transaction
        .attach_background_job(execution, background_job_id)
        .await
}

pub(super) async fn advance_schedule(
    transaction: &dyn JobScheduleTransaction,
    mut schedule: JobScheduleRecord,
    next_run_at: DateTime<Utc>,
    last_run_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    schedule.next_run_at = Some(next_run_at);
    schedule.last_run_at = Some(last_run_at);
    schedule.updated_at = now;
    transaction.save_schedule(schedule).await?;
    Ok(())
}

pub(super) async fn quarantine_invalid_schedule(
    transaction: &dyn JobScheduleTransaction,
    mut schedule: JobScheduleRecord,
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
            trigger_kind: TRIGGER_SCHEDULED,
            scheduled_for,
            outcome: OUTCOME_INVALID_CONFIGURATION,
            detail: Some(detail.to_owned()),
            created_at: now,
        },
    )
    .await?;

    schedule.enabled = false;
    schedule.next_run_at = None;
    schedule.version = schedule.version.saturating_add(1);
    schedule.updated_at = now;
    transaction.save_schedule(schedule).await?;
    Ok(())
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
        &[TRIGGER_SCHEDULED, TRIGGER_MISFIRE, TRIGGER_MANUAL],
        "触发类型",
    )
}

pub(super) fn normalize_outcome(value: Option<String>) -> AppResult<Option<String>> {
    normalize_enum_filter(
        value,
        &[
            OUTCOME_ENQUEUED,
            OUTCOME_SKIPPED_MISFIRE,
            OUTCOME_SKIPPED_CONCURRENCY,
            OUTCOME_TARGET_UNAVAILABLE,
            OUTCOME_INVALID_CONFIGURATION,
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

pub(super) async fn rollback_with<T>(
    transaction: Box<dyn JobScheduleTransaction>,
    error: AppError,
) -> AppResult<T> {
    if let Err(rollback_error) = transaction.rollback().await {
        tracing::warn!(%rollback_error, "回滚调度事务失败");
    }
    Err(error)
}
