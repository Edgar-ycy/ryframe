use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_adapters::repository::{PageResult, ValidatedPageQuery};
use ryframe_config::{DataRetentionConfig, TenantConfigTransferConfig};
use ryframe_db::{
    ControlDatabaseCluster, DataRetentionRepository, EnqueueBackgroundJob, FileRepository,
    RetentionCleanupResult, RetentionCutoff, RetentionResource, TenantRepository,
    UserImportRepository, background_job, data_retention_run, tenant_config_bundle,
    tenant_config_transfer,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
    sea_query::{Expr, LockType, SimpleExpr},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{JobHandler, JobQueue, system::FileService};

pub const DATA_RETENTION_JOB_TYPE: &str = "system.data_retention.cleanup";

mod config_transfer;
mod handler;
mod imports;
mod resources;
mod support;
mod workflow;

pub use handler::DataRetentionJobHandler;

use support::*;

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
    pub tenant_config_artifact_hours: u32,
    pub tenant_config_rollback_hours: u32,
    pub retention_run_days: u32,
    pub service_access_audit_days: u32,
    pub dead_background_jobs_permanent: bool,
    pub dead_outbox_events_permanent: bool,
}

impl DataRetentionPolicy {
    fn from_configs(
        config: &DataRetentionConfig,
        tenant_config: &TenantConfigTransferConfig,
    ) -> Self {
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
            tenant_config_artifact_hours: tenant_config.artifact_hours,
            tenant_config_rollback_hours: tenant_config.rollback_hours,
            retention_run_days: config.retention_run_days,
            service_access_audit_days: config.service_access_audit_days,
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
    db: ControlDatabaseCluster,
    repository: Arc<DataRetentionRepository>,
    queue: Arc<JobQueue>,
    file_service: Arc<FileService>,
    config: DataRetentionConfig,
    tenant_config: TenantConfigTransferConfig,
}

impl DataRetentionService {
    pub fn new(
        db: ControlDatabaseCluster,
        queue: Arc<JobQueue>,
        file_service: Arc<FileService>,
        config: DataRetentionConfig,
        tenant_config: TenantConfigTransferConfig,
    ) -> Self {
        Self {
            db,
            repository: Arc::new(DataRetentionRepository),
            queue,
            file_service,
            config,
            tenant_config,
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
        eligible_counts.extend(self.preview_tenant_config_artifacts(calculated_at).await?);
        Ok(DataRetentionPreview {
            calculated_at,
            policy: DataRetentionPolicy::from_configs(&self.config, &self.tenant_config),
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

    pub(super) fn overview_at(&self, calculated_at: DateTime<Utc>) -> DataRetentionOverview {
        let cutoffs = self.cutoffs(calculated_at);
        let mut cutoff_views = cutoff_views(&cutoffs);
        cutoff_views.push(DataRetentionCutoffVo {
            resource: "user_import_artifacts".to_owned(),
            before: self.import_artifact_cutoff(calculated_at),
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
        DataRetentionOverview {
            calculated_at,
            policy: DataRetentionPolicy::from_configs(&self.config, &self.tenant_config),
            cutoffs: cutoff_views,
        }
    }
}
