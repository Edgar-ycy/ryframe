mod actions;
mod copy;
mod models;
mod queries;
mod recovery;
mod support;
mod workflow;

use std::sync::Arc;

use chrono::Duration;
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde_json::json;

use crate::{
    AuthorizationCache, EnqueueJob, JobQueue,
    ports::tenant_data::{
        CreateTenantDataMigrationRecord, TenantDataMigrationPersistencePort,
        TenantDataMigrationPort, TenantDataMigrationRecord, TenantDataPlacementRecord,
        TenantDataTargetPort, TenantOperationLeaseRecord,
    },
};

pub use copy::rolling_digest;
pub use models::*;
pub use workflow::TenantDataMigrationJobHandler;

use support::{
    MigrationPlanHashInput, create_blocker_error, ensure_platform_actor,
    ensure_same_create_request, migration_plan_hash, sha256_hex, target_summary,
    validate_idempotency_key, validate_migration_tenant,
};

pub const TENANT_DATA_MIGRATION_JOB_TYPE: &str = "tenant_data_migration";
const RETENTION_HOURS: i32 = 168;
const OPERATION_LEASE_HOURS: i64 = 24;

pub(super) fn checked_generation(value: i64, field: &str) -> AppResult<i64> {
    (value > 0)
        .then_some(value)
        .ok_or_else(|| AppError::Conflict(format!("{field} generation 无效")))
}

#[derive(Clone)]
pub struct TenantDataMigrationService {
    pub(super) persistence: Arc<dyn TenantDataMigrationPersistencePort>,
    targets: Arc<dyn TenantDataTargetPort>,
    pub(super) tenant_migration: Arc<dyn TenantDataMigrationPort>,
    pub(super) queue: Arc<JobQueue>,
    pub(super) authorization_cache: AuthorizationCache,
}

impl TenantDataMigrationService {
    pub fn new(
        persistence: Arc<dyn TenantDataMigrationPersistencePort>,
        targets: Arc<dyn TenantDataTargetPort>,
        tenant_migration: Arc<dyn TenantDataMigrationPort>,
        queue: Arc<JobQueue>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            persistence,
            targets,
            tenant_migration,
            queue,
            authorization_cache,
        }
    }

    pub async fn list_targets(&self, actor: &ActorContext) -> AppResult<Vec<DataTargetSummary>> {
        ensure_platform_actor(actor)?;
        Ok(self
            .targets
            .metadata()
            .await?
            .into_iter()
            .map(target_summary)
            .collect())
    }

    pub async fn list_targets_with_context(
        &self,
        actor: &ActorContext,
        params: DataTargetListParams,
    ) -> AppResult<Vec<DataTargetSummary>> {
        let mut targets = self.list_targets(actor).await?;
        if let Some(query) = params
            .q
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
        {
            if query.chars().count() > 100 {
                return Err(AppError::Validation(
                    "数据目标搜索关键字最多 100 个字符".into(),
                ));
            }
            let query = query.to_lowercase();
            targets.retain(|target| {
                [
                    Some(target.key.as_str()),
                    target.display_name.as_deref(),
                    target.region.as_deref(),
                    Some(target.mode.as_str()),
                    Some(target.kind.as_str()),
                    Some(target.health.as_str()),
                ]
                .into_iter()
                .flatten()
                .any(|value| value.to_lowercase().contains(&query))
            });
        }
        let Some(eligible_for) = params.eligible_for.as_deref() else {
            return Ok(targets);
        };
        if !matches!(eligible_for, "new_tenant" | "migration") {
            return Err(AppError::Validation(
                "eligible_for 只允许 new_tenant 或 migration".into(),
            ));
        }
        let configured_target_keys = targets
            .iter()
            .map(|target| target.key.clone())
            .collect::<Vec<_>>();
        let occupied = self
            .persistence
            .occupied_target_keys(&configured_target_keys)
            .await?;
        if eligible_for == "migration" {
            let tenant_id = params
                .tenant_id
                .as_deref()
                .ok_or_else(|| AppError::Validation("迁移目标筛选必须提供 tenant_id".into()))?;
            let placement = self
                .persistence
                .placement(tenant_id)
                .await?
                .ok_or_else(|| AppError::NotFound("租户数据 placement 不存在".into()))?;
            for target in &mut targets {
                if target.key == placement.current_target_key {
                    target.eligible = false;
                    target.reasons.push("source_equals_target".into());
                }
                if self.targets.is_dedicated(&target.key) == Some(true)
                    && occupied.contains(&target.key)
                {
                    target.eligible = false;
                    target.reasons.push("dedicated_target_occupied".into());
                }
                target.reasons.sort_unstable();
                target.reasons.dedup();
            }
            return Ok(targets);
        }

        for target in &mut targets {
            if self.targets.is_dedicated(&target.key) == Some(true)
                && occupied.contains(&target.key)
            {
                target.eligible = false;
                target.reasons.push("dedicated_target_occupied".into());
            }
        }
        Ok(targets)
    }

    pub async fn target_detail(
        &self,
        actor: &ActorContext,
        target_key: &str,
    ) -> AppResult<DataTargetDetail> {
        ensure_platform_actor(actor)?;
        if !self.targets.contains(target_key) {
            return Err(AppError::NotFound("数据目标不存在".into()));
        }
        // 详情页允许显式探测；列表始终只读缓存，避免 N 个目标同步放大。
        let _ = self.targets.verify_now(target_key).await;
        let metadata = self
            .targets
            .metadata()
            .await?
            .into_iter()
            .find(|target| target.key == target_key)
            .ok_or_else(|| AppError::NotFound("数据目标不存在".into()))?;
        let last_verified_at = metadata.last_verified_at;
        let target = target_summary(metadata);
        let pool = self.targets.pool_stats().await?;
        Ok(DataTargetDetail {
            target,
            last_verified_at,
            reserved_connections: pool.reserved_connections,
            max_total_connections: pool.max_total_connections,
            open_targets: pool.open_targets,
            opening_targets: pool.opening_targets,
        })
    }

    pub async fn backup_points(
        &self,
        actor: &ActorContext,
        target_key: &str,
        params: BackupPointListParams,
    ) -> AppResult<Vec<BackupPointView>> {
        ensure_platform_actor(actor)?;
        if !self.targets.contains(target_key) {
            return Err(AppError::NotFound("数据目标不存在".into()));
        }
        self.persistence
            .backup_points_for_target(
                target_key,
                params.tenant_id.as_deref(),
                params.limit.clamp(1, 200),
            )
            .await
            .map(|rows| rows.into_iter().map(BackupPointView::from).collect())
    }

    pub async fn placement(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
    ) -> AppResult<DataPlacementView> {
        ensure_platform_actor(actor)?;
        self.persistence
            .placement(tenant_id)
            .await?
            .map(DataPlacementView::from)
            .ok_or_else(|| AppError::NotFound("租户数据 placement 不存在".into()))
    }

    pub async fn preview(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        request: MigrationPreviewRequest,
    ) -> AppResult<MigrationPreview> {
        ensure_platform_actor(actor)?;
        validate_migration_tenant(tenant_id)?;
        let expected_generation =
            checked_generation(request.expected_placement_generation, "placement")?;
        let placement = self
            .persistence
            .placement(tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据 placement 不存在".into()))?;
        if placement.placement_generation != expected_generation {
            return Err(AppError::StalePlacementGeneration(
                "租户数据 placement generation 已变化".into(),
            ));
        }

        let target_generation = request
            .expected_placement_generation
            .checked_add(1)
            .ok_or_else(|| AppError::Validation("placement generation 已达上限".into()))?;
        let mut blockers = Vec::new();
        let mut warnings = vec!["stop_write_required".to_owned()];
        if placement.state != TenantDataPlacementRecord::STATE_ACTIVE {
            blockers.push("placement_not_active".to_owned());
        }
        if placement.current_target_key == request.target_key {
            blockers.push("source_equals_target".to_owned());
        }
        if !self.targets.contains(&request.target_key) {
            blockers.push("target_not_registered".to_owned());
        }
        if self
            .persistence
            .active_migration_for_tenant(tenant_id)
            .await?
            .is_some()
        {
            blockers.push("tenant_operation_in_progress".to_owned());
        }

        if !self.targets.contains(&placement.current_target_key) {
            blockers.push("source_target_not_registered".to_owned());
        } else if self
            .targets
            .validate_catalog(&placement.current_target_key)
            .await
            .is_err()
        {
            blockers.push("source_target_unavailable".to_owned());
        }
        if self.targets.contains(&request.target_key) {
            match self.targets.validate_catalog(&request.target_key).await {
                Ok(target) => {
                    if target.dedicated {
                        match self.targets.is_occupied(&request.target_key).await {
                            Ok(true) => blockers.push("dedicated_target_occupied".to_owned()),
                            Ok(false) => {}
                            Err(_) => blockers.push("target_occupancy_unavailable".to_owned()),
                        }
                    }
                    match self
                        .targets
                        .tenant_is_empty(&request.target_key, tenant_id)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => blockers.push("target_tenant_data_not_empty".to_owned()),
                        Err(_) => blockers.push("target_empty_check_unavailable".to_owned()),
                    }
                }
                Err(_) => blockers.push("target_unavailable".to_owned()),
            }
        }
        blockers.sort_unstable();
        blockers.dedup();
        warnings.sort_unstable();
        let plan_hash = migration_plan_hash(MigrationPlanHashInput {
            tenant_id,
            source_target_key: &placement.current_target_key,
            target_key: &request.target_key,
            source_generation: request.expected_placement_generation,
            target_generation,
            source_mode: self.targets.mode_code(&placement.current_target_key),
            source_kind: self.targets.kind_code(&placement.current_target_key),
            target_mode: self.targets.mode_code(&request.target_key),
            target_kind: self.targets.kind_code(&request.target_key),
            schema_fingerprint: &self.targets.catalog_fingerprint(),
        });
        Ok(MigrationPreview {
            tenant_id: tenant_id.to_owned(),
            source_target_key: placement.current_target_key,
            target_target_key: request.target_key,
            expected_placement_generation: request.expected_placement_generation.to_string(),
            target_generation: target_generation.to_string(),
            plan_hash,
            eligible: blockers.is_empty(),
            blockers,
            warnings,
            impact: MigrationImpact {
                stop_write: true,
                catalog_table_count: self.targets.catalog_table_count(),
                retention_hours: RETENTION_HOURS,
                rollback_boundary: "before_cutting_over".into(),
            },
        })
    }

    pub async fn create(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        command: CreateMigrationCommand,
    ) -> AppResult<MigrationView> {
        ensure_platform_actor(actor)?;
        validate_idempotency_key(&command.idempotency_key)?;
        let create_key_hash = sha256_hex(&format!(
            "ryframe:tenant-data:create:v1:{tenant_id}:{}",
            command.idempotency_key
        ));
        if let Some(existing) = self
            .persistence
            .migration_by_create_key(&create_key_hash)
            .await?
        {
            ensure_same_create_request(&existing, &command)?;
            return self.migration_view(existing).await;
        }

        let preview = self
            .validated_create_preview(actor, tenant_id, &command)
            .await?;

        let migration_id = crate::next_id()?;
        let now = self.persistence.database_now().await?;
        let target_generation = command
            .expected_placement_generation
            .checked_add(1)
            .ok_or_else(|| AppError::Validation("placement generation 已达上限".into()))?;
        let switch_token = sha256_hex(&format!(
            "ryframe:tenant-data:migration:v1:{tenant_id}:{migration_id}:{}:{target_generation}",
            command.target_key
        ));
        let source_mode = self
            .targets
            .mode_code(&preview.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let source_kind = self
            .targets
            .kind_code(&preview.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let target_mode = self
            .targets
            .mode_code(&command.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let target_kind = self
            .targets
            .kind_code(&command.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let transaction = self.persistence.begin().await?;
        if let Err(error) = transaction
            .acquire_lease(TenantOperationLeaseRecord {
                tenant_id: tenant_id.to_owned(),
                owner_token: switch_token.clone(),
                operation: "tenant_data.migration".into(),
                resource_type: "tenant_data_migration".into(),
                resource_id: migration_id.to_string(),
                expires_at: now + Duration::hours(OPERATION_LEASE_HOURS),
                created_at: now,
                updated_at: now,
            })
            .await
        {
            let _ = transaction.rollback().await;
            if let Some(existing) = self
                .persistence
                .migration_by_create_key(&create_key_hash)
                .await?
            {
                ensure_same_create_request(&existing, &command)?;
                return self.migration_view(existing).await;
            }
            return Err(error);
        }
        let placement = transaction.lock_placement(tenant_id).await?;
        validate_create_placement(&placement, &preview, &command)?;
        if transaction
            .lock_active_migration_for_tenant(tenant_id)
            .await?
            .is_some()
        {
            return Err(AppError::TenantOperationConflict(
                "租户已有未完成的数据迁移".into(),
            ));
        }

        let inserted = transaction
            .insert_migration(CreateTenantDataMigrationRecord {
                id: migration_id,
                tenant_id: tenant_id.to_owned(),
                source_target_key: placement.current_target_key,
                target_key: command.target_key.clone(),
                source_target_mode: source_mode.into(),
                source_target_kind: source_kind.into(),
                target_target_mode: target_mode.into(),
                target_target_kind: target_kind.into(),
                source_generation: placement.placement_generation,
                source_switch_token: placement.switch_token,
                target_generation,
                source_schema_fingerprint: self.targets.catalog_fingerprint(),
                target_schema_fingerprint: self.targets.catalog_fingerprint(),
                plan_hash: command.plan_hash.clone(),
                create_idempotency_key_hash: create_key_hash.clone(),
                switch_token: switch_token.clone(),
                operator_id: actor.user_id,
                retention_hours: RETENTION_HOURS,
                now,
            })
            .await;
        let mut migration = match inserted {
            Ok(migration) => migration,
            Err(error) => {
                let _ = transaction.rollback().await;
                if let Some(existing) = self
                    .persistence
                    .migration_by_create_key(&create_key_hash)
                    .await?
                {
                    ensure_same_create_request(&existing, &command)?;
                    return self.migration_view(existing).await;
                }
                return Err(error);
            }
        };
        let queued = self
            .enqueue_migration_job(transaction.as_ref(), tenant_id, migration_id, now)
            .await;
        let queued = match queued {
            Ok(queued) => queued,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        migration.background_job_id = Some(queued);
        migration.updated_at = now;
        migration = transaction.save_migration(migration).await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        self.queue.notify_background_jobs().await;
        self.migration_view(migration).await
    }

    async fn validated_create_preview(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        command: &CreateMigrationCommand,
    ) -> AppResult<MigrationPreview> {
        let preview = self
            .preview(
                actor,
                tenant_id,
                MigrationPreviewRequest {
                    target_key: command.target_key.clone(),
                    expected_placement_generation: command.expected_placement_generation,
                },
            )
            .await?;
        if !preview.eligible {
            return Err(create_blocker_error(&preview.blockers));
        }
        if preview.plan_hash != command.plan_hash {
            return Err(AppError::Conflict("迁移预览 plan_hash 已失效".into()));
        }

        Ok(preview)
    }

    async fn enqueue_migration_job(
        &self,
        transaction: &dyn crate::ports::tenant_data::TenantDataMigrationTransaction,
        tenant_id: &str,
        migration_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<i64> {
        self.queue
            .enqueue_in_transaction(
                transaction.background_jobs(),
                EnqueueJob {
                    tenant_id: Some(tenant_id.to_owned()),
                    schedule_id: None,
                    scheduled_for: None,
                    max_runtime_seconds: Some(86_400),
                    job_type: TENANT_DATA_MIGRATION_JOB_TYPE.into(),
                    payload: json!({ "migration_id": migration_id.to_string() }),
                    priority: 10,
                    available_at: now,
                    max_attempts: 8,
                    dedupe_key: Some(migration_id.to_string()),
                    traceparent: None,
                    tracestate: None,
                },
            )
            .await
            .map(|queued| queued.job_id)
    }
}

fn validate_create_placement(
    placement: &TenantDataPlacementRecord,
    preview: &MigrationPreview,
    command: &CreateMigrationCommand,
) -> AppResult<()> {
    if placement.state != TenantDataPlacementRecord::STATE_ACTIVE
        || placement.current_target_key != preview.source_target_key
        || placement.placement_generation
            != checked_generation(command.expected_placement_generation, "placement")?
    {
        return Err(AppError::StalePlacementGeneration(
            "租户数据 placement 已变化".into(),
        ));
    }
    Ok(())
}
