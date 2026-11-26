use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use ryframe_kernel::{ActorContext, AppError, AppResult, PageResult, ValidatedPageQuery};
use serde::Serialize;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime, UtcOffset};

use crate::{
    ClaimedBackgroundJob, EnqueueJob, JobHandler, JobQueue,
    ports::retention::{
        RetentionCleanupPersistencePort, RetentionCleanupResult, RetentionCutoff,
        RetentionResource, RetentionRunPersistencePort, RetentionRunRecord,
    },
    ports::tenant_config::{
        TENANT_CONFIG_PACKAGE_RESOURCE, TENANT_CONFIG_SNAPSHOT_RESOURCE,
        TenantConfigArtifactCounts, TenantConfigRetentionPersistencePort,
    },
    ports::utc_timestamp,
    system::file::FileService,
};

pub const DATA_RETENTION_JOB_TYPE: &str = "system.data_retention.cleanup";

mod config_transfer;
mod handler;
mod imports;
mod resources;
mod support;
mod workflow;

pub use handler::DataRetentionJobHandler;

use support::*;

#[derive(Clone, Copy, Debug, Serialize)]
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
    pub tenant_config_artifact_hours: u32,
    pub tenant_config_rollback_hours: u32,
    pub retention_run_days: u32,
    pub dead_background_jobs_permanent: bool,
    pub dead_outbox_events_permanent: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionCutoffVo {
    pub resource: String,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    pub before: OffsetDateTime,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionOverview {
    #[serde(serialize_with = "utc_timestamp::serialize")]
    pub calculated_at: OffsetDateTime,
    pub policy: DataRetentionPolicy,
    pub cutoffs: Vec<DataRetentionCutoffVo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DataRetentionPreview {
    #[serde(serialize_with = "utc_timestamp::serialize")]
    pub calculated_at: OffsetDateTime,
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    pub updated_at: OffsetDateTime,
}

impl TryFrom<RetentionRunRecord> for DataRetentionRunVo {
    type Error = AppError;

    fn try_from(model: RetentionRunRecord) -> AppResult<Self> {
        let to_utc = |timestamp: OffsetDateTime, field| {
            timestamp.checked_to_offset(UtcOffset::UTC).ok_or_else(|| {
                AppError::Internal(format!("数据保留运行记录的 {field} 无法精确表示为 UTC"))
            })
        };
        Ok(Self {
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
            started_at: model
                .started_at
                .map(|value| to_utc(value, "started_at"))
                .transpose()?,
            completed_at: model
                .completed_at
                .map(|value| to_utc(value, "completed_at"))
                .transpose()?,
            created_at: to_utc(model.created_at, "created_at")?,
            updated_at: to_utc(model.updated_at, "updated_at")?,
        })
    }
}

#[derive(Clone)]
pub struct DataRetentionService {
    config_artifacts: Arc<dyn TenantConfigRetentionPersistencePort>,
    cleanup_persistence: Arc<dyn RetentionCleanupPersistencePort>,
    run_persistence: Arc<dyn RetentionRunPersistencePort>,
    queue: Arc<JobQueue>,
    file: Arc<FileService>,
    config: DataRetentionPolicy,
}

impl DataRetentionService {
    pub fn new(
        config_artifacts: Arc<dyn TenantConfigRetentionPersistencePort>,
        cleanup_persistence: Arc<dyn RetentionCleanupPersistencePort>,
        run_persistence: Arc<dyn RetentionRunPersistencePort>,
        queue: Arc<JobQueue>,
        file: Arc<FileService>,
        config: DataRetentionPolicy,
    ) -> Self {
        Self {
            config_artifacts,
            cleanup_persistence,
            run_persistence,
            queue,
            file,
            config,
        }
    }

    pub async fn overview(&self, actor: &ActorContext) -> AppResult<DataRetentionOverview> {
        ensure_system_tenant(actor)?;
        let calculated_at = self.run_persistence.database_now().await?;
        self.overview_at(calculated_at)
    }

    pub async fn preview(&self, actor: &ActorContext) -> AppResult<DataRetentionPreview> {
        ensure_system_tenant(actor)?;
        let calculated_at = self.run_persistence.database_now().await?;
        let cutoffs = self.cutoffs(calculated_at)?;
        let mut eligible_counts = self.cleanup_persistence.preview(&cutoffs, None).await?;
        eligible_counts.insert(
            "user_import_artifacts".to_owned(),
            self.cleanup_persistence
                .count_expired_import_artifacts(self.import_artifact_cutoff(calculated_at)?)
                .await?,
        );
        eligible_counts.extend(self.preview_tenant_config_artifacts(calculated_at).await?);
        Ok(DataRetentionPreview {
            calculated_at,
            policy: self.config,
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
        let transaction = self.run_persistence.begin().await?;
        let result = async {
            let now = transaction.database_now().await?;
            let proposed_run_id = crate::next_id()?;
            let trace_context = crate::trace_context::current_trace_context();
            let enqueue = self
                .queue
                .enqueue_in_transaction(
                    transaction.background_jobs(),
                    EnqueueJob {
                        tenant_id: None,
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: Some(900),
                        job_type: DATA_RETENTION_JOB_TYPE.to_owned(),
                        payload: json!({
                            "run_id": proposed_run_id.to_string(),
                            "trigger_kind": RetentionRunRecord::TRIGGER_MANUAL,
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
            let existing = transaction.find_by_background_job(enqueue.job_id).await?;
            let run = if let Some(existing) = existing {
                existing
            } else {
                transaction
                    .insert_if_missing(new_run_record(
                        proposed_run_id,
                        enqueue.job_id,
                        RetentionRunRecord::TRIGGER_MANUAL,
                        Some(actor.user_id),
                        now,
                    ))
                    .await?
            };
            DataRetentionRunVo::try_from(run)
        }
        .await;
        match result {
            Ok(run) => {
                transaction
                    .commit(crate::TransactionAuditMode::Skip)
                    .await?;
                self.queue.notify_background_jobs().await;
                Ok(run)
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
        let result = self.run_persistence.list(page).await?;
        Ok(PageResult {
            records: result
                .records
                .into_iter()
                .map(DataRetentionRunVo::try_from)
                .collect::<AppResult<_>>()?,
            total: result.total,
            page: result.page,
            page_size: result.page_size,
        })
    }

    pub(super) fn overview_at(
        &self,
        calculated_at: OffsetDateTime,
    ) -> AppResult<DataRetentionOverview> {
        let cutoffs = self.cutoffs(calculated_at)?;
        let mut cutoff_views = cutoff_views(&cutoffs);
        cutoff_views.push(DataRetentionCutoffVo {
            resource: "user_import_artifacts".to_owned(),
            before: self.import_artifact_cutoff(calculated_at)?,
        });
        cutoff_views.push(DataRetentionCutoffVo {
            resource: "tenant_config_packages".to_owned(),
            // 配置包在创建时已经把保留时长折算为 expires_at；这里展示实际比较基准。
            before: calculated_at,
        });
        cutoff_views.push(DataRetentionCutoffVo {
            resource: "tenant_config_snapshots".to_owned(),
            // 回滚窗口同样由 rollback_expires_at 表达，清理条件是截止时间不晚于当前时刻。
            before: calculated_at,
        });
        Ok(DataRetentionOverview {
            calculated_at,
            policy: self.config,
            cutoffs: cutoff_views,
        })
    }
}
