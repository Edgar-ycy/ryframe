use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_config::DataRetentionConfig;
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    DataRetentionRepository, DatabaseCluster, EnqueueBackgroundJob, RetentionCutoff,
    RetentionResource, UserImportRepository, background_job, data_retention_run,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    JobHandler, JobQueue,
    system::{FileService, IMPORT_BUCKET},
};

pub const DATA_RETENTION_JOB_TYPE: &str = "system.data_retention.cleanup";

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionPolicy {
    pub cleanup_batch_size: usize,
    pub max_rows_per_resource_per_run: usize,
    pub background_job_succeeded_days: u32,
    pub outbox_published_days: u32,
    pub schedule_execution_days: u32,
    pub export_job_history_days: u32,
    pub operation_log_days: u32,
    pub login_log_days: u32,
    pub user_import_history_days: u32,
    pub user_import_artifact_hours: u32,
    pub retention_run_days: u32,
    pub dead_background_jobs_permanent: bool,
    pub dead_outbox_events_permanent: bool,
}

impl From<&DataRetentionConfig> for DataRetentionPolicy {
    fn from(config: &DataRetentionConfig) -> Self {
        Self {
            cleanup_batch_size: config.cleanup_batch_size,
            max_rows_per_resource_per_run: config.max_rows_per_resource_per_run,
            background_job_succeeded_days: config.background_job_succeeded_days,
            outbox_published_days: config.outbox_published_days,
            schedule_execution_days: config.schedule_execution_days,
            export_job_history_days: config.export_job_history_days,
            operation_log_days: config.operation_log_days,
            login_log_days: config.login_log_days,
            user_import_history_days: config.user_import_history_days,
            user_import_artifact_hours: config.user_import_artifact_hours,
            retention_run_days: config.retention_run_days,
            dead_background_jobs_permanent: true,
            dead_outbox_events_permanent: true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionCutoffVo {
    pub resource: String,
    pub before: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionOverview {
    pub calculated_at: DateTime<Utc>,
    pub policy: DataRetentionPolicy,
    pub cutoffs: Vec<DataRetentionCutoffVo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionPreview {
    pub calculated_at: DateTime<Utc>,
    pub policy: DataRetentionPolicy,
    pub cutoffs: Vec<DataRetentionCutoffVo>,
    pub eligible_counts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionRunVo {
    pub id: String,
    pub background_job_id: String,
    pub trigger_kind: String,
    pub status: String,
    pub policy_snapshot: Value,
    pub eligible_counts: Value,
    pub deleted_counts: Value,
    pub remaining_counts: Value,
    pub requested_by: Option<String>,
    pub error_summary: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<data_retention_run::Model> for DataRetentionRunVo {
    fn from(model: data_retention_run::Model) -> Self {
        Self {
            id: model.id.to_string(),
            background_job_id: model.background_job_id.to_string(),
            trigger_kind: model.trigger_kind,
            status: model.status,
            policy_snapshot: model.policy_snapshot,
            eligible_counts: model.eligible_counts,
            deleted_counts: model.deleted_counts,
            remaining_counts: model.remaining_counts,
            requested_by: model.requested_by.map(|id| id.to_string()),
            error_summary: model.error_summary,
            started_at: model.started_at,
            completed_at: model.completed_at,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

#[derive(Clone)]
pub struct DataRetentionService {
    db: DatabaseCluster,
    repository: Arc<DataRetentionRepository>,
    queue: Arc<JobQueue>,
    file_service: Arc<FileService>,
    config: DataRetentionConfig,
}

impl DataRetentionService {
    pub fn new(
        db: DatabaseCluster,
        queue: Arc<JobQueue>,
        file_service: Arc<FileService>,
        config: DataRetentionConfig,
    ) -> Self {
        Self {
            db,
            repository: Arc::new(DataRetentionRepository),
            queue,
            file_service,
            config,
        }
    }

    pub async fn overview(&self, actor: &ActorContext) -> AppResult<DataRetentionOverview> {
        ensure_system_tenant(actor)?;
        let calculated_at = self.repository.database_utc_now(self.db.write()).await?;
        Ok(self.overview_at(calculated_at))
    }

    pub async fn preview(&self, actor: &ActorContext) -> AppResult<DataRetentionPreview> {
        ensure_system_tenant(actor)?;
        let calculated_at = self.repository.database_utc_now(self.db.write()).await?;
        let cutoffs = self.cutoffs(calculated_at);
        let mut eligible_counts = self
            .repository
            .preview(self.db.write(), &cutoffs, None)
            .await?;
        eligible_counts.insert(
            "user_import_artifacts".to_owned(),
            UserImportRepository
                .count_expired_artifacts(
                    self.db.write(),
                    self.import_artifact_cutoff(calculated_at),
                )
                .await?,
        );
        Ok(DataRetentionPreview {
            calculated_at,
            policy: DataRetentionPolicy::from(&self.config),
            cutoffs: cutoff_views(&cutoffs),
            eligible_counts,
        })
    }

    pub async fn enqueue_manual(
        &self,
        actor: &ActorContext,
        idempotency_key_hash: &str,
    ) -> AppResult<DataRetentionRunVo> {
        ensure_system_tenant(actor)?;
        if idempotency_key_hash.len() != 64 {
            return Err(AppError::Validation("Idempotency-Key 摘要无效".into()));
        }
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let result = async {
            let now = self.repository.database_utc_now(&transaction).await?;
            let proposed_run_id = ryframe_utils::snowflake::try_next_snowflake_id()?;
            let trace_context = crate::trace_context::current_trace_context();
            let enqueue = self
                .queue
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: None,
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(900),
                        job_type: DATA_RETENTION_JOB_TYPE.to_owned(),
                        payload: json!({
                            "run_id": proposed_run_id.to_string(),
                            "trigger_kind": data_retention_run::Model::TRIGGER_MANUAL,
                            "requested_by": actor.user_id.to_string(),
                        }),
                        priority: -20,
                        available_at: now,
                        max_attempts: 20,
                        dedupe_key: Some(format!("manual:{idempotency_key_hash}")),
                        traceparent: trace_context.traceparent,
                        tracestate: trace_context.tracestate,
                    },
                )
                .await?;
            let existing = self
                .repository
                .find_run_by_background_job(&transaction, enqueue.job.id)
                .await?;
            let run = if let Some(existing) = existing {
                existing
            } else {
                self.repository
                    .insert_run_if_missing(
                        &transaction,
                        new_run_model(
                            proposed_run_id,
                            enqueue.job.id,
                            data_retention_run::Model::TRIGGER_MANUAL,
                            Some(actor.user_id),
                            now,
                        ),
                    )
                    .await?
            };
            Ok::<_, AppError>(run)
        }
        .await;
        match result {
            Ok(run) => {
                crate::commit_current_audit(transaction).await?;
                self.queue.notify_background_jobs().await;
                Ok(run.into())
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    pub async fn list_runs(
        &self,
        actor: &ActorContext,
        page: ValidatedPageQuery,
    ) -> AppResult<PageResult<DataRetentionRunVo>> {
        ensure_system_tenant(actor)?;
        let result = self.repository.list_runs(self.db.write(), &page).await?;
        Ok(PageResult {
            records: result
                .records
                .into_iter()
                .map(DataRetentionRunVo::from)
                .collect(),
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub async fn execute_job(&self, job: &background_job::Model) -> AppResult<()> {
        let now = self.repository.database_utc_now(self.db.write()).await?;
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(Value::as_str)
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
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok());
        let proposed_id = job
            .payload
            .get("run_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(ryframe_utils::snowflake::try_next_snowflake_id()?);
        let mut run = self
            .repository
            .insert_run_if_missing(
                self.db.write(),
                new_run_model(proposed_id, job.id, trigger_kind, requested_by, now),
            )
            .await?;

        run.status = data_retention_run::Model::STATUS_RUNNING.to_owned();
        run.started_at.get_or_insert(now);
        run.completed_at = None;
        run.error_summary = None;
        run.updated_at = now;
        let overview = self.overview_at(now);
        run.policy_snapshot = serde_json::to_value(&overview).map_err(json_error)?;
        let cutoffs = self.cutoffs(now);
        let mut eligible = self
            .repository
            .preview(self.db.write(), &cutoffs, Some(run.id))
            .await?;
        eligible.insert(
            "user_import_artifacts".to_owned(),
            UserImportRepository
                .count_expired_artifacts(self.db.write(), self.import_artifact_cutoff(now))
                .await?,
        );
        run.eligible_counts = serde_json::to_value(&eligible).map_err(json_error)?;
        run = self.repository.update_run(self.db.write(), run).await?;

        let mut deleted = json_counts(&run.deleted_counts);
        let mut remaining = BTreeMap::new();
        match self.cleanup_import_artifacts(now).await {
            Ok(result) => {
                *deleted
                    .entry("user_import_artifacts".to_owned())
                    .or_default() += result.deleted;
                remaining.insert("user_import_artifacts".to_owned(), result.remaining);
                run.deleted_counts = serde_json::to_value(&deleted).map_err(json_error)?;
                run.remaining_counts = serde_json::to_value(&remaining).map_err(json_error)?;
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run = self.repository.update_run(self.db.write(), run).await?;
            }
            Err(error) => {
                run.status = data_retention_run::Model::STATUS_FAILED.to_owned();
                run.error_summary = Some(safe_error_summary(&error));
                run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                run.completed_at = Some(run.updated_at);
                let _ = self.repository.update_run(self.db.write(), run).await;
                return Err(error);
            }
        }
        for cutoff in cutoffs {
            match self
                .repository
                .cleanup_resource(
                    self.db.write(),
                    cutoff,
                    self.config.cleanup_batch_size,
                    self.config.max_rows_per_resource_per_run,
                    Some(run.id),
                )
                .await
            {
                Ok(result) => {
                    *deleted.entry(cutoff.resource.key().to_owned()).or_default() += result.deleted;
                    remaining.insert(cutoff.resource.key().to_owned(), result.remaining);
                    run.deleted_counts = serde_json::to_value(&deleted).map_err(json_error)?;
                    run.remaining_counts = serde_json::to_value(&remaining).map_err(json_error)?;
                    run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                    run = self.repository.update_run(self.db.write(), run).await?;
                }
                Err(error) => {
                    run.status = data_retention_run::Model::STATUS_FAILED.to_owned();
                    run.error_summary = Some(safe_error_summary(&error));
                    run.updated_at = self.repository.database_utc_now(self.db.write()).await?;
                    run.completed_at = Some(run.updated_at);
                    let _ = self.repository.update_run(self.db.write(), run).await;
                    return Err(error);
                }
            }
        }
        let completed_at = self.repository.database_utc_now(self.db.write()).await?;
        run.status = if remaining.values().any(|count| *count > 0) {
            data_retention_run::Model::STATUS_PARTIAL
        } else {
            data_retention_run::Model::STATUS_SUCCEEDED
        }
        .to_owned();
        run.completed_at = Some(completed_at);
        run.updated_at = completed_at;
        self.repository.update_run(self.db.write(), run).await?;
        Ok(())
    }

    fn overview_at(&self, calculated_at: DateTime<Utc>) -> DataRetentionOverview {
        let cutoffs = self.cutoffs(calculated_at);
        let mut cutoff_views = cutoff_views(&cutoffs);
        cutoff_views.push(DataRetentionCutoffVo {
            resource: "user_import_artifacts".to_owned(),
            before: self.import_artifact_cutoff(calculated_at),
        });
        DataRetentionOverview {
            calculated_at,
            policy: DataRetentionPolicy::from(&self.config),
            cutoffs: cutoff_views,
        }
    }

    fn import_artifact_cutoff(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now - Duration::hours(i64::from(self.config.user_import_artifact_hours))
    }

    async fn cleanup_import_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<ryframe_db::RetentionCleanupResult> {
        let before = self.import_artifact_cutoff(now);
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        let mut after_id = None;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let artifacts = UserImportRepository
                .list_expired_artifacts_after_id(self.db.write(), before, after_id, limit)
                .await?;
            if artifacts.is_empty() {
                break;
            }
            let batch_len = artifacts.len();
            for artifact in artifacts {
                after_id = Some(artifact.file_id);
                if self
                    .file_service
                    .delete_internal(&artifact.tenant_id, artifact.file_id, IMPORT_BUCKET)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = UserImportRepository
            .count_expired_artifacts(self.db.write(), before)
            .await?;
        Ok(ryframe_db::RetentionCleanupResult { deleted, remaining })
    }

    fn cutoffs(&self, now: DateTime<Utc>) -> Vec<RetentionCutoff> {
        vec![
            cutoff(
                RetentionResource::BackgroundJobs,
                now,
                self.config.background_job_succeeded_days,
            ),
            cutoff(
                RetentionResource::OutboxEvents,
                now,
                self.config.outbox_published_days,
            ),
            cutoff(
                RetentionResource::ScheduleExecutions,
                now,
                self.config.schedule_execution_days,
            ),
            cutoff(
                RetentionResource::ExportJobs,
                now,
                self.config.export_job_history_days,
            ),
            cutoff(
                RetentionResource::OperationLogs,
                now,
                self.config.operation_log_days,
            ),
            cutoff(
                RetentionResource::LoginLogs,
                now,
                self.config.login_log_days,
            ),
            cutoff(
                RetentionResource::UserImports,
                now,
                self.config.user_import_history_days,
            ),
            cutoff(
                RetentionResource::RetentionRuns,
                now,
                self.config.retention_run_days,
            ),
        ]
    }
}

pub struct DataRetentionJobHandler {
    service: Arc<DataRetentionService>,
}

impl DataRetentionJobHandler {
    pub fn new(service: Arc<DataRetentionService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for DataRetentionJobHandler {
    fn job_type(&self) -> &'static str {
        DATA_RETENTION_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        self.service.execute_job(job).await
    }
}

fn new_run_model(
    id: i64,
    background_job_id: i64,
    trigger_kind: &str,
    requested_by: Option<i64>,
    now: DateTime<Utc>,
) -> data_retention_run::Model {
    data_retention_run::Model {
        id,
        background_job_id,
        trigger_kind: trigger_kind.to_owned(),
        status: data_retention_run::Model::STATUS_PENDING.to_owned(),
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

fn cutoff(resource: RetentionResource, now: DateTime<Utc>, days: u32) -> RetentionCutoff {
    RetentionCutoff {
        resource,
        before: now - Duration::days(i64::from(days)),
    }
}

fn cutoff_views(cutoffs: &[RetentionCutoff]) -> Vec<DataRetentionCutoffVo> {
    cutoffs
        .iter()
        .map(|cutoff| DataRetentionCutoffVo {
            resource: cutoff.resource.key().to_owned(),
            before: cutoff.before,
        })
        .collect()
}

fn ensure_system_tenant(actor: &ActorContext) -> AppResult<()> {
    if crate::validated_tenant_id(actor)? == "system" {
        Ok(())
    } else {
        Err(AppError::Authorization("数据保留只允许系统租户访问".into()))
    }
}

fn json_counts(value: &Value) -> BTreeMap<String, u64> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn safe_error_summary(error: &AppError) -> String {
    error.to_string().chars().take(500).collect()
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

fn json_error(error: serde_json::Error) -> AppError {
    AppError::Internal(format!("数据保留汇总编码失败: {error}"))
}
