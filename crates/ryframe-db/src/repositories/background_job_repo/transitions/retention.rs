use super::*;

impl BackgroundJobRepository {
    pub(super) async fn ensure_retention_run<C>(
        db: &C,
        job: &background_job::Model,
        now: DateTime<Utc>,
    ) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        if data_retention_run::Entity::find()
            .filter(data_retention_run::Column::BackgroundJobId.eq(job.id))
            .one(db)
            .await
            .map_err(database_error)?
            .is_some()
        {
            return Ok(());
        }
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    data_retention_run::Model::TRIGGER_MANUAL
                        | data_retention_run::Model::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(data_retention_run::Model::TRIGGER_SCHEDULED);
        let requested_by = job
            .payload
            .get("requested_by")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<i64>().ok());
        let run_id = job
            .payload
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(job.id);
        DataRetentionRepository
            .insert_run_if_missing(
                db,
                data_retention_run::Model {
                    id: run_id,
                    background_job_id: job.id,
                    trigger_kind: trigger_kind.to_owned(),
                    status: data_retention_run::Model::STATUS_PENDING.to_owned(),
                    policy_snapshot: serde_json::json!({}),
                    eligible_counts: serde_json::json!({}),
                    deleted_counts: serde_json::json!({}),
                    remaining_counts: serde_json::json!({}),
                    requested_by,
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        Ok(())
    }
}
