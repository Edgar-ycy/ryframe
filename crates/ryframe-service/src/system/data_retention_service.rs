use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_config::{DataRetentionConfig, TenantConfigTransferConfig};
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    DataRetentionRepository, DatabaseCluster, EnqueueBackgroundJob, FileRepository,
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

fn config_bundle_not_required_by_active_transfer() -> SimpleExpr {
    // 只保护已经入队或正在执行的预览、应用操作；尚未提交和已完成的预览会随源包到期失效。
    // `pending_preview` 是旧版本中无法区分“待用户操作/已入队”的状态，滚动升级期间按活跃状态保守保护。
    Expr::cust(
        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_transfer transfer WHERE transfer.tenant_id = sys_tenant_config_bundle.tenant_id AND transfer.bundle_id = sys_tenant_config_bundle.id AND transfer.status IN ('pending_preview', 'preview_pending', 'previewing', 'apply_pending', 'applying'))",
    )
}

fn config_snapshot_not_used_by_active_rollback() -> SimpleExpr {
    Expr::cust("sys_tenant_config_transfer.status NOT IN ('rollback_pending', 'rolling_back')")
}

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
    db: DatabaseCluster,
    repository: Arc<DataRetentionRepository>,
    queue: Arc<JobQueue>,
    file_service: Arc<FileService>,
    config: DataRetentionConfig,
    tenant_config: TenantConfigTransferConfig,
}

impl DataRetentionService {
    pub fn new(
        db: DatabaseCluster,
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
        self.repository
            .insert_run_if_missing(
                self.db.write(),
                new_run_model(proposed_id, job.id, trigger_kind, requested_by, now),
            )
            .await?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let run = self
            .repository
            .lock_run_by_background_job_in_txn(&transaction, job.id)
            .await?
            .ok_or_else(|| AppError::NotFound("数据保留运行记录不存在".into()))?;
        // 后台任务可能在业务清理已经提交后丢失租约，并由管理员重新投递。完成态是可靠事实，
        // 通过行锁原子确认后直接返回，避免再次执行永久删除或重写原完成时间。
        let Some(mut run) = self
            .repository
            .begin_run_in_txn(&transaction, run, now)
            .await?
        else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(());
        };
        transaction.commit().await.map_err(database_error)?;
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
        eligible.extend(self.preview_tenant_config_artifacts(now).await?);
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
        match self.cleanup_tenant_config_artifacts(now).await {
            Ok(counts) => {
                for (resource, count) in counts {
                    *deleted.entry(resource.clone()).or_default() += count.deleted;
                    remaining.insert(resource, count.remaining);
                }
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

    /// 在 Worker 领取后先建立运行记录，使外层超时或租约恢复也能同步公开状态。
    pub async fn prepare_job(&self, job: &background_job::Model) -> AppResult<()> {
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
        self.repository
            .insert_run_if_missing(
                self.db.write(),
                new_run_model(proposed_id, job.id, trigger_kind, requested_by, now),
            )
            .await?;
        Ok(())
    }

    fn overview_at(&self, calculated_at: DateTime<Utc>) -> DataRetentionOverview {
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
                    .delete_expired_import_artifact(&artifact.tenant_id, artifact.file_id, before)
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

    async fn preview_tenant_config_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<BTreeMap<String, u64>> {
        let packages = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::FileId.is_not_null())
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(now))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        let snapshots = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(now))
            .filter(config_snapshot_not_used_by_active_rollback())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(BTreeMap::from([
            ("tenant_config_packages".to_owned(), packages),
            ("tenant_config_snapshots".to_owned(), snapshots),
        ]))
    }

    async fn cleanup_tenant_config_artifacts(
        &self,
        now: DateTime<Utc>,
    ) -> AppResult<BTreeMap<String, RetentionCleanupResult>> {
        let packages = self.cleanup_expired_config_packages(now).await?;
        let snapshots = self.cleanup_expired_config_snapshots(now).await?;
        Ok(BTreeMap::from([
            ("tenant_config_packages".to_owned(), packages),
            ("tenant_config_snapshots".to_owned(), snapshots),
        ]))
    }

    async fn cleanup_expired_config_packages(
        &self,
        before: DateTime<Utc>,
    ) -> AppResult<RetentionCleanupResult> {
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let candidates = tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::FileId.is_not_null())
                .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
                .filter(tenant_config_bundle::Column::Status.is_in([
                    tenant_config_bundle::Model::STATUS_SUCCEEDED,
                    tenant_config_bundle::Model::STATUS_FAILED,
                    tenant_config_bundle::Model::STATUS_EXPIRED,
                ]))
                .filter(config_bundle_not_required_by_active_transfer())
                .order_by_asc(tenant_config_bundle::Column::ExpiresAt)
                .order_by_asc(tenant_config_bundle::Column::Id)
                .limit(u64::try_from(limit).unwrap_or(u64::MAX))
                .all(self.db.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self
                    .detach_expired_config_package(candidate, before)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::FileId.is_not_null())
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    async fn detach_expired_config_package(
        &self,
        candidate: tenant_config_bundle::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.file_id else {
            return Ok(false);
        };
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, &candidate.tenant_id)
            .await?;
        let Some(current) = tenant_config_bundle::Entity::find_by_id(candidate.id)
            .filter(tenant_config_bundle::Column::TenantId.eq(&candidate.tenant_id))
            .filter(tenant_config_bundle::Column::FileId.eq(file_id))
            .filter(tenant_config_bundle::Column::ExpiresAt.lte(before))
            .filter(tenant_config_bundle::Column::Status.is_in([
                tenant_config_bundle::Model::STATUS_SUCCEEDED,
                tenant_config_bundle::Model::STATUS_FAILED,
                tenant_config_bundle::Model::STATUS_EXPIRED,
            ]))
            .filter(config_bundle_not_required_by_active_transfer())
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(false);
        };
        let mut active: tenant_config_bundle::ActiveModel = current.into();
        active.file_id = Set(None);
        active.status = Set(tenant_config_bundle::Model::STATUS_EXPIRED.to_owned());
        let now = self.repository.database_utc_now(&transaction).await?;
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        // 同一内容的内部文件可能由多个配置包或快照共享。当前业务引用到期后始终
        // 提交解绑；只有最后一个引用消失时，仓储更新才会把物理文件置为清理墓碑。
        let _marked_for_cleanup = FileRepository
            .mark_unreferenced_config_package_for_cleanup_in_txn(
                &transaction,
                &candidate.tenant_id,
                file_id,
                now,
                now + Duration::minutes(15),
            )
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
    }

    async fn cleanup_expired_config_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> AppResult<RetentionCleanupResult> {
        let maximum = self.config.max_rows_per_resource_per_run;
        let mut deleted = 0_u64;
        while usize::try_from(deleted).unwrap_or(usize::MAX) < maximum {
            let remaining_limit =
                maximum.saturating_sub(usize::try_from(deleted).unwrap_or(usize::MAX));
            let limit = self.config.cleanup_batch_size.min(remaining_limit);
            if limit == 0 {
                break;
            }
            let candidates = tenant_config_transfer::Entity::find()
                .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
                .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
                .filter(config_snapshot_not_used_by_active_rollback())
                .order_by_asc(tenant_config_transfer::Column::RollbackExpiresAt)
                .order_by_asc(tenant_config_transfer::Column::Id)
                .limit(u64::try_from(limit).unwrap_or(u64::MAX))
                .all(self.db.write())
                .await
                .map_err(database_error)?;
            if candidates.is_empty() {
                break;
            }
            let batch_len = candidates.len();
            for candidate in candidates {
                if self
                    .detach_expired_config_snapshot(candidate, before)
                    .await?
                {
                    deleted = deleted.saturating_add(1);
                }
            }
            if batch_len < limit {
                break;
            }
        }
        let remaining = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::SnapshotFileId.is_not_null())
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
            .filter(config_snapshot_not_used_by_active_rollback())
            .count(self.db.write())
            .await
            .map_err(database_error)?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    async fn detach_expired_config_snapshot(
        &self,
        candidate: tenant_config_transfer::Model,
        before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let Some(file_id) = candidate.snapshot_file_id else {
            return Ok(false);
        };
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        TenantRepository
            .lock_tenant_in_txn(&transaction, &candidate.tenant_id)
            .await?;
        let Some(current) = tenant_config_transfer::Entity::find_by_id(candidate.id)
            .filter(tenant_config_transfer::Column::TenantId.eq(&candidate.tenant_id))
            .filter(tenant_config_transfer::Column::SnapshotFileId.eq(file_id))
            .filter(tenant_config_transfer::Column::RollbackExpiresAt.lte(before))
            .filter(config_snapshot_not_used_by_active_rollback())
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            transaction.rollback().await.map_err(database_error)?;
            return Ok(false);
        };
        let mut active: tenant_config_transfer::ActiveModel = current.into();
        active.snapshot_file_id = Set(None);
        let now = self.repository.database_utc_now(&transaction).await?;
        active.updated_at = Set(now);
        active.update(&transaction).await.map_err(database_error)?;
        // 快照也可能与其他记录复用同一文件；先可靠解除到期引用，最后一个引用
        // 的事务负责把文件置为清理墓碑，其他事务不应因此让整轮保留任务失败。
        let _marked_for_cleanup = FileRepository
            .mark_unreferenced_config_package_for_cleanup_in_txn(
                &transaction,
                &candidate.tenant_id,
                file_id,
                now,
                now + Duration::minutes(15),
            )
            .await?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
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
                RetentionResource::ServiceAccessAudits,
                now,
                self.config.service_access_audit_days,
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
        self.service.prepare_job(job).await?;
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
