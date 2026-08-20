use super::*;

pub(super) fn new_run_record(
    id: i64,
    background_job_id: i64,
    trigger_kind: &str,
    requested_by: Option<i64>,
    now: DateTime<Utc>,
) -> RetentionRunRecord {
    RetentionRunRecord {
        id,
        background_job_id,
        trigger_kind: trigger_kind.to_owned(),
        status: RetentionRunRecord::STATUS_PENDING.to_owned(),
        policy_snapshot: json!({}),
        eligible_counts: json!({}),
        deleted_counts: json!({}),
        remaining_counts: json!({}),
        requested_by,
        error_summary: None,
        started_at: None,
        completed_at: None,
        created_at: now,
        updated_at: now,
    }
}

pub(super) fn cutoff(
    resource: RetentionResource,
    now: DateTime<Utc>,
    days: u32,
) -> RetentionCutoff {
    RetentionCutoff {
        resource,
        before: now - Duration::days(i64::from(days)),
    }
}

pub(super) fn cutoff_views(cutoffs: &[RetentionCutoff]) -> Vec<DataRetentionCutoffVo> {
    cutoffs
        .iter()
        .map(|cutoff| DataRetentionCutoffVo {
            resource: cutoff.resource.key().to_owned(),
            before: cutoff.before,
        })
        .collect()
}

pub(super) fn ensure_system_tenant(actor: &ActorContext) -> AppResult<()> {
    if crate::validated_tenant_id(actor)? == "system" {
        Ok(())
    } else {
        Err(AppError::Authorization("数据保留只允许系统租户访问".into()))
    }
}

pub(super) fn json_counts(value: &Value) -> BTreeMap<String, u64> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

pub(super) fn safe_error_summary(error: &AppError) -> String {
    error.to_string().chars().take(500).collect()
}

pub(super) fn json_error(error: serde_json::Error) -> AppError {
    AppError::Internal(format!("数据保留汇总编码失败: {error}"))
}
