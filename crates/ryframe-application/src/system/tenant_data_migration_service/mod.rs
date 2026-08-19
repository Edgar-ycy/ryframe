mod actions;
mod copy;
mod models;
mod recovery;
mod workflow;

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use ryframe_config::{TenantDatabaseTargetKind, TenantDatabaseTargetMode};
use ryframe_db::{
    ControlDatabaseCluster, CreateTenantDataMigration, EnqueueBackgroundJob, TenantDataRepository,
    TenantOperationLeaseRepository, tenant_data_backup_point, tenant_data_migration,
    tenant_data_migration_item, tenant_operation_lease,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_tenant_db::{
    TenantDatabaseRouter, TenantDatabaseTargetHealthStatus, TenantDatabaseTargetMetadata,
};
use sea_orm::TransactionTrait;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{AuthorizationCache, JobQueue};

pub use models::*;
pub use workflow::TenantDataMigrationJobHandler;

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
    pub(super) database: ControlDatabaseCluster,
    pub(super) router: Arc<TenantDatabaseRouter>,
    pub(super) queue: Arc<JobQueue>,
    pub(super) authorization_cache: AuthorizationCache,
    pub(super) repository: TenantDataRepository,
    pub(super) lease_repository: Arc<TenantOperationLeaseRepository>,
    pub(super) catalog: ryframe_tenant_db::migration::TenantDataCatalog,
}

impl TenantDataMigrationService {
    pub fn new(
        database: ControlDatabaseCluster,
        router: Arc<TenantDatabaseRouter>,
        queue: Arc<JobQueue>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            database,
            router,
            queue,
            authorization_cache,
            repository: TenantDataRepository,
            lease_repository: Arc::new(TenantOperationLeaseRepository),
            catalog: ryframe_tenant_db::migration::TENANT_DATA_CATALOG,
        }
    }

    /// 仅用于针对复制/恢复算法的隔离集成测试；生产组合根始终使用编译期 catalog。
    #[doc(hidden)]
    pub fn with_catalog_for_tests(
        mut self,
        catalog: ryframe_tenant_db::migration::TenantDataCatalog,
    ) -> AppResult<Self> {
        catalog.validate_structure().map_err(AppError::Config)?;
        self.catalog = catalog;
        Ok(self)
    }

    pub async fn list_targets(&self, actor: &ActorContext) -> AppResult<Vec<DataTargetSummary>> {
        ensure_platform_actor(actor)?;
        Ok(self
            .router
            .targets()
            .metadata()
            .await
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
            .repository
            .occupied_target_keys(self.database.write(), &configured_target_keys)
            .await?;
        if eligible_for == "migration" {
            let tenant_id = params
                .tenant_id
                .as_deref()
                .ok_or_else(|| AppError::Validation("迁移目标筛选必须提供 tenant_id".into()))?;
            let placement = self
                .repository
                .placement(self.database.write(), tenant_id)
                .await?
                .ok_or_else(|| AppError::NotFound("租户数据 placement 不存在".into()))?;
            for target in &mut targets {
                if target.key == placement.current_target_key {
                    target.eligible = false;
                    target.reasons.push("source_equals_target".into());
                }
                if self.router.targets().target_mode(&target.key)
                    == Some(TenantDatabaseTargetMode::Dedicated)
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
            if self.router.targets().target_mode(&target.key)
                == Some(TenantDatabaseTargetMode::Dedicated)
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
        if !self.router.targets().contains(target_key) {
            return Err(AppError::NotFound("数据目标不存在".into()));
        }
        // 详情页允许显式探测；列表始终只读缓存，避免 N 个目标同步放大。
        let _ = self.router.verify_target_now(target_key).await;
        let metadata = self
            .router
            .targets()
            .metadata()
            .await
            .into_iter()
            .find(|target| target.key == target_key)
            .ok_or_else(|| AppError::NotFound("数据目标不存在".into()))?;
        let last_verified_at = metadata.last_verified_at.map(DateTime::<Utc>::from);
        let target = target_summary(metadata);
        let pool = self.router.targets().pool_stats().await;
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
        if !self.router.targets().contains(target_key) {
            return Err(AppError::NotFound("数据目标不存在".into()));
        }
        self.repository
            .backup_points_for_target(
                self.database.write(),
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
        self.repository
            .placement(self.database.write(), tenant_id)
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
            .repository
            .placement(self.database.write(), tenant_id)
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
        if placement.state != ryframe_db::tenant_data_placement::Model::STATE_ACTIVE {
            blockers.push("placement_not_active".to_owned());
        }
        if placement.current_target_key == request.target_key {
            blockers.push("source_equals_target".to_owned());
        }
        if !self.router.targets().contains(&request.target_key) {
            blockers.push("target_not_registered".to_owned());
        }
        if self
            .repository
            .active_migration_for_tenant(self.database.write(), tenant_id)
            .await?
            .is_some()
        {
            blockers.push("tenant_operation_in_progress".to_owned());
        }

        if !self
            .router
            .targets()
            .contains(&placement.current_target_key)
        {
            blockers.push("source_target_not_registered".to_owned());
        } else if self
            .router
            .open_target_for_catalog(&placement.current_target_key, &self.catalog)
            .await
            .is_err()
        {
            blockers.push("source_target_unavailable".to_owned());
        }
        if self.router.targets().contains(&request.target_key) {
            match self
                .router
                .open_target_for_catalog(&request.target_key, &self.catalog)
                .await
            {
                Ok(target) => {
                    if target.mode() == TenantDatabaseTargetMode::Dedicated {
                        match self
                            .router
                            .target_occupancy_for_catalog(&request.target_key, &self.catalog)
                            .await
                        {
                            Ok(Some(_)) => blockers.push("dedicated_target_occupied".to_owned()),
                            Ok(None) => {}
                            Err(_) => blockers.push("target_occupancy_unavailable".to_owned()),
                        }
                    }
                    match self
                        .router
                        .tenant_is_empty_on_target_for_catalog(
                            &request.target_key,
                            tenant_id,
                            &self.catalog,
                        )
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
            source_mode: self
                .router
                .targets()
                .target_mode(&placement.current_target_key),
            source_kind: self
                .router
                .targets()
                .target_kind(&placement.current_target_key),
            target_mode: self.router.targets().target_mode(&request.target_key),
            target_kind: self.router.targets().target_kind(&request.target_key),
            schema_fingerprint: &self.catalog.schema_fingerprint(),
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
                catalog_table_count: self.catalog.tables().len(),
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
            .repository
            .migration_by_create_key(self.database.write(), &create_key_hash)
            .await?
        {
            ensure_same_create_request(&existing, &command)?;
            return self.migration_view(existing).await;
        }

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

        let migration_id = ryframe_adapters::snowflake::try_next_snowflake_id()?;
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let target_generation = command
            .expected_placement_generation
            .checked_add(1)
            .ok_or_else(|| AppError::Validation("placement generation 已达上限".into()))?;
        let switch_token = sha256_hex(&format!(
            "ryframe:tenant-data:migration:v1:{tenant_id}:{migration_id}:{}:{target_generation}",
            command.target_key
        ));
        let source_mode = self
            .router
            .targets()
            .target_mode(&preview.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let source_kind = self
            .router
            .targets()
            .target_kind(&preview.source_target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("源目标未注册".into(), 5))?;
        let target_mode = self
            .router
            .targets()
            .target_mode(&command.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let target_kind = self
            .router
            .targets()
            .target_kind(&command.target_key)
            .ok_or_else(|| AppError::TenantDataTargetUnavailable("目标未注册".into(), 5))?;
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        if let Err(error) = self
            .lease_repository
            .acquire_in_txn(
                &transaction,
                tenant_operation_lease::Model {
                    tenant_id: tenant_id.to_owned(),
                    owner_token: switch_token.clone(),
                    operation: "tenant_data.migration".into(),
                    resource_type: "tenant_data_migration".into(),
                    resource_id: migration_id.to_string(),
                    expires_at: now + Duration::hours(OPERATION_LEASE_HOURS),
                    created_at: now,
                    updated_at: now,
                },
            )
            .await
        {
            let _ = transaction.rollback().await;
            if let Some(existing) = self
                .repository
                .migration_by_create_key(self.database.write(), &create_key_hash)
                .await?
            {
                ensure_same_create_request(&existing, &command)?;
                return self.migration_view(existing).await;
            }
            return Err(error);
        }
        let placement = self
            .repository
            .lock_placement_in_txn(&transaction, tenant_id)
            .await?;
        if placement.state != ryframe_db::tenant_data_placement::Model::STATE_ACTIVE
            || placement.current_target_key != preview.source_target_key
            || placement.placement_generation
                != checked_generation(command.expected_placement_generation, "placement")?
        {
            return Err(AppError::StalePlacementGeneration(
                "租户数据 placement 已变化".into(),
            ));
        }
        if self
            .repository
            .lock_active_migration_for_tenant_in_txn(&transaction, tenant_id)
            .await?
            .is_some()
        {
            return Err(AppError::TenantOperationConflict(
                "租户已有未完成的数据迁移".into(),
            ));
        }

        let inserted = self
            .repository
            .insert_migration_in_txn(
                &transaction,
                CreateTenantDataMigration {
                    id: migration_id,
                    tenant_id: tenant_id.to_owned(),
                    source_target_key: placement.current_target_key,
                    target_key: command.target_key.clone(),
                    source_target_mode: target_mode_code(source_mode).into(),
                    source_target_kind: target_kind_code(source_kind).into(),
                    target_target_mode: target_mode_code(target_mode).into(),
                    target_target_kind: target_kind_code(target_kind).into(),
                    source_generation: placement.placement_generation,
                    source_switch_token: placement.switch_token,
                    target_generation,
                    source_schema_fingerprint: self.catalog.schema_fingerprint(),
                    target_schema_fingerprint: self.catalog.schema_fingerprint(),
                    plan_hash: command.plan_hash.clone(),
                    create_idempotency_key_hash: create_key_hash.clone(),
                    switch_token: switch_token.clone(),
                    operator_id: actor.user_id,
                    retention_hours: RETENTION_HOURS,
                    now,
                },
            )
            .await;
        let mut migration = match inserted {
            Ok(migration) => migration,
            Err(error) => {
                let _ = transaction.rollback().await;
                if let Some(existing) = self
                    .repository
                    .migration_by_create_key(self.database.write(), &create_key_hash)
                    .await?
                {
                    ensure_same_create_request(&existing, &command)?;
                    return self.migration_view(existing).await;
                }
                return Err(error);
            }
        };
        let queued = self
            .queue
            .enqueue_in_transaction(
                &transaction,
                EnqueueBackgroundJob {
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
            .await;
        let queued = match queued {
            Ok(queued) => queued,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        migration.background_job_id = Some(queued.job.id);
        migration.updated_at = now;
        migration = self
            .repository
            .save_migration_in_txn(&transaction, migration)
            .await?;
        transaction.commit().await.map_err(database_error)?;
        self.queue.notify_background_jobs().await;
        self.migration_view(migration).await
    }

    pub async fn migration(
        &self,
        actor: &ActorContext,
        migration_id: i64,
    ) -> AppResult<MigrationView> {
        ensure_platform_actor(actor)?;
        let migration = self
            .repository
            .migration(self.database.write(), migration_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))?;
        self.migration_view(migration).await
    }

    pub async fn migrations_for_tenant(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        limit: u64,
    ) -> AppResult<Vec<MigrationView>> {
        ensure_platform_actor(actor)?;
        validate_migration_tenant(tenant_id)?;
        let rows = self
            .repository
            .migrations_for_tenant(self.database.write(), tenant_id, limit.clamp(1, 100))
            .await?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            views.push(self.migration_view(row).await?);
        }
        Ok(views)
    }

    pub(super) async fn migration_view(
        &self,
        migration: tenant_data_migration::Model,
    ) -> AppResult<MigrationView> {
        let items = self
            .repository
            .items(self.database.write(), migration.id)
            .await?;
        let now = self
            .lease_repository
            .database_utc_now(self.database.write())
            .await?;
        let mut can_finalize = false;
        let mut action_reasons = Vec::new();
        if migration.can_finalize() {
            if migration.retention_until.is_none_or(|until| until > now) {
                action_reasons.push("retention_period_not_elapsed".into());
            } else if let Some(not_before) = migration.activated_at.or(migration.succeeded_at) {
                let backup = self
                    .repository
                    .validated_backup_for_destination(
                        self.database.write(),
                        &migration,
                        not_before,
                        now,
                    )
                    .await?;
                if backup.is_some() {
                    can_finalize = true;
                } else {
                    action_reasons.push("validated_backup_required".into());
                }
            } else {
                action_reasons.push("activation_timestamp_missing".into());
            }
        } else {
            action_reasons.push("migration_not_retention_pending".into());
        }
        let can_cancel = migration.can_cancel()
            && migration.cancel_requested_at.is_none()
            && migration.error_code.is_none();
        if !can_cancel {
            action_reasons.push(if migration.cancel_requested_at.is_some() {
                "cancel_requested".into()
            } else {
                "migration_past_cancel_boundary".into()
            });
        }
        if migration.finalize_requested_at.is_some()
            && migration.state == tenant_data_migration::Model::STATE_RETENTION_PENDING
        {
            can_finalize = false;
            action_reasons.push("finalize_requested".into());
        }
        Ok(MigrationView::from_models(
            migration,
            items,
            can_cancel,
            can_finalize,
            action_reasons,
        ))
    }
}

fn ensure_platform_actor(actor: &ActorContext) -> AppResult<()> {
    if actor.tenant_id != "system" {
        return Err(AppError::Authorization(
            "数据放置平台仅允许 system 租户访问".into(),
        ));
    }
    Ok(())
}

fn validate_migration_tenant(tenant_id: &str) -> AppResult<()> {
    // 数据迁移由 system 租户的控制面操作员发起，目标租户天然不同于请求主体。
    // 此处不能使用 validate_explicit_tenant，否则请求上下文会错误地拒绝所有
    // 非 system 租户的迁移列表、预览和创建操作。
    ryframe_adapters::validate_tenant_identifier(tenant_id)?;
    if tenant_id == "system" {
        return Err(AppError::Validation("system 租户禁止迁移业务数据".into()));
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> AppResult<()> {
    if key != key.trim() || key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "Idempotency-Key 必须为 1–128 位可打印字符".into(),
        ));
    }
    Ok(())
}

fn ensure_same_create_request(
    migration: &tenant_data_migration::Model,
    command: &CreateMigrationCommand,
) -> AppResult<()> {
    if migration.target_key == command.target_key
        && migration.plan_hash == command.plan_hash
        && migration.source_generation == command.expected_placement_generation
    {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "Idempotency-Key 已用于不同的数据迁移请求".into(),
        ))
    }
}

fn create_blocker_error(blockers: &[String]) -> AppError {
    if blockers
        .iter()
        .any(|blocker| blocker == "placement_not_active")
    {
        return AppError::TenantDataMaintenance("租户业务数据当前不可迁移".into(), 5);
    }
    if blockers.iter().any(|blocker| {
        matches!(
            blocker.as_str(),
            "tenant_operation_in_progress" | "dedicated_target_occupied"
        )
    }) {
        return AppError::TenantOperationConflict("租户或专属目标正在执行其他操作".into());
    }
    if blockers.iter().any(|blocker| {
        matches!(
            blocker.as_str(),
            "source_target_not_registered"
                | "source_target_unavailable"
                | "target_unavailable"
                | "target_occupancy_unavailable"
                | "target_empty_check_unavailable"
        )
    }) {
        return AppError::TenantDataTargetUnavailable("租户数据目标当前不可用".into(), 5);
    }
    AppError::Validation(format!("租户数据迁移不可执行: {}", blockers.join(",")))
}

fn target_summary(metadata: TenantDatabaseTargetMetadata) -> DataTargetSummary {
    let health = match metadata.health {
        TenantDatabaseTargetHealthStatus::Unknown => "unknown",
        TenantDatabaseTargetHealthStatus::Verified => "verified",
        TenantDatabaseTargetHealthStatus::Unavailable => "unavailable",
    };
    let mut reasons = Vec::new();
    if metadata.health != TenantDatabaseTargetHealthStatus::Verified {
        reasons.push(
            if metadata.health == TenantDatabaseTargetHealthStatus::Unavailable {
                "target_unavailable".into()
            } else {
                "target_not_verified".into()
            },
        );
    }
    DataTargetSummary {
        key: metadata.key,
        display_name: metadata.display_name,
        mode: match metadata.mode {
            TenantDatabaseTargetMode::Shared => "shared".into(),
            TenantDatabaseTargetMode::Dedicated => "dedicated".into(),
        },
        kind: match metadata.kind {
            TenantDatabaseTargetKind::Control => "control".into(),
            TenantDatabaseTargetKind::Mysql => "mysql".into(),
        },
        region: metadata.region,
        health: health.into(),
        schema_fingerprint: metadata.schema_fingerprint,
        connected: metadata.connected,
        pool_max_connections: metadata.pool_max_connections,
        active_leases: metadata.active_leases,
        eligible: reasons.is_empty(),
        reasons,
    }
}

const fn target_mode_code(mode: TenantDatabaseTargetMode) -> &'static str {
    match mode {
        TenantDatabaseTargetMode::Shared => "shared",
        TenantDatabaseTargetMode::Dedicated => "dedicated",
    }
}

const fn target_kind_code(kind: TenantDatabaseTargetKind) -> &'static str {
    match kind {
        TenantDatabaseTargetKind::Control => "control",
        TenantDatabaseTargetKind::Mysql => "mysql",
    }
}

struct MigrationPlanHashInput<'a> {
    tenant_id: &'a str,
    source_target_key: &'a str,
    target_key: &'a str,
    source_generation: i64,
    target_generation: i64,
    source_mode: Option<TenantDatabaseTargetMode>,
    source_kind: Option<TenantDatabaseTargetKind>,
    target_mode: Option<TenantDatabaseTargetMode>,
    target_kind: Option<TenantDatabaseTargetKind>,
    schema_fingerprint: &'a str,
}

fn migration_plan_hash(input: MigrationPlanHashInput<'_>) -> String {
    let MigrationPlanHashInput {
        tenant_id,
        source_target_key,
        target_key,
        source_generation,
        target_generation,
        source_mode,
        source_kind,
        target_mode,
        target_kind,
        schema_fingerprint,
    } = input;
    let source_mode = match source_mode {
        Some(TenantDatabaseTargetMode::Shared) => "shared",
        Some(TenantDatabaseTargetMode::Dedicated) => "dedicated",
        None => "unknown",
    };
    let source_kind = match source_kind {
        Some(TenantDatabaseTargetKind::Control) => "control",
        Some(TenantDatabaseTargetKind::Mysql) => "mysql",
        None => "unknown",
    };
    let target_mode = match target_mode {
        Some(TenantDatabaseTargetMode::Shared) => "shared",
        Some(TenantDatabaseTargetMode::Dedicated) => "dedicated",
        None => "unknown",
    };
    let target_kind = match target_kind {
        Some(TenantDatabaseTargetKind::Control) => "control",
        Some(TenantDatabaseTargetKind::Mysql) => "mysql",
        None => "unknown",
    };
    sha256_hex(&format!(
        "ryframe:tenant-data:plan:v1|tenant={tenant_id}|source={source_target_key}|target={target_key}|source_generation={source_generation}|target_generation={target_generation}|schema={schema_fingerprint}|source_mode={source_mode}|source_kind={source_kind}|target_mode={target_mode}|target_kind={target_kind}|migration_mode=stop_write|retention_hours={RETENTION_HOURS}",
    ))
}

pub(super) fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

impl From<tenant_data_backup_point::Model> for BackupPointView {
    fn from(model: tenant_data_backup_point::Model) -> Self {
        Self {
            id: model.id.to_string(),
            scope: model.scope,
            tenant_id: model.tenant_id,
            target_key: model.target_key,
            placement_generation: model.placement_generation.map(|value| value.to_string()),
            schema_fingerprint: model.schema_fingerprint,
            captured_at: model.captured_at,
            checksum: model.checksum,
            validation_status: model.validation_status,
            retention_until: model.retention_until,
            expires_at: model.expires_at,
            last_restore_drill_at: model.last_restore_drill_at,
        }
    }
}

impl From<ryframe_db::tenant_data_placement::Model> for DataPlacementView {
    fn from(model: ryframe_db::tenant_data_placement::Model) -> Self {
        Self {
            tenant_id: model.tenant_id,
            current_target_key: model.current_target_key,
            placement_generation: model.placement_generation.to_string(),
            state: model.state,
            updated_at: model.updated_at,
        }
    }
}

impl MigrationView {
    fn from_models(
        migration: tenant_data_migration::Model,
        items: Vec<tenant_data_migration_item::Model>,
        can_cancel: bool,
        can_finalize: bool,
        action_reasons: Vec<String>,
    ) -> Self {
        let cancel_requested = migration.cancel_requested_at.is_some();
        let finalize_requested = migration.finalize_requested_at.is_some();
        Self {
            id: migration.id.to_string(),
            tenant_id: migration.tenant_id,
            source_target_key: migration.source_target_key,
            target_target_key: migration.target_key,
            source_generation: migration.source_generation.to_string(),
            target_generation: migration.target_generation.to_string(),
            source_schema_fingerprint: migration.source_schema_fingerprint,
            target_schema_fingerprint: migration.target_schema_fingerprint,
            plan_hash: migration.plan_hash,
            state: migration.state,
            operator_id: migration.operator_id.to_string(),
            retention_hours: migration.retention_hours,
            error_code: migration.error_code,
            prechecked_at: migration.prechecked_at,
            queued_at: migration.queued_at,
            quiesced_at: migration.quiesced_at,
            frozen_at: migration.frozen_at,
            copy_started_at: migration.copy_started_at,
            copy_completed_at: migration.copy_completed_at,
            verified_at: migration.verified_at,
            cut_over_at: migration.cut_over_at,
            activated_at: migration.activated_at,
            succeeded_at: migration.succeeded_at,
            retention_until: migration.retention_until,
            finalized_at: migration.finalized_at,
            failed_at: migration.failed_at,
            cancelled_at: migration.cancelled_at,
            created_at: migration.created_at,
            updated_at: migration.updated_at,
            can_cancel,
            can_finalize,
            cancel_requested,
            finalize_requested,
            action_reasons,
            items: items.into_iter().map(MigrationItemView::from).collect(),
        }
    }
}

impl From<tenant_data_migration_item::Model> for MigrationItemView {
    fn from(model: tenant_data_migration_item::Model) -> Self {
        Self {
            id: model.id.to_string(),
            table_name: model.table_name,
            copy_order: model.copy_order,
            state: model.state,
            cursor: model.cursor_json,
            source_row_count: model.source_row_count.map(|value| value.to_string()),
            target_row_count: model.target_row_count.map(|value| value.to_string()),
            source_digest: model.source_digest,
            target_digest: model.target_digest,
            error_code: model.error_code,
            cleanup_state: model.cleanup_state,
            cleanup_row_count: model.cleanup_row_count.to_string(),
        }
    }
}
