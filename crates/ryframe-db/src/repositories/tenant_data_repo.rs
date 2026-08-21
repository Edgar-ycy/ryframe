use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
    sea_query::{Condition, LockType},
};
use std::collections::HashSet;

use crate::entities::{
    tenant_data_backup_point, tenant_data_migration, tenant_data_migration_item,
    tenant_data_placement,
};

#[derive(Clone, Debug)]
pub struct CreateTenantDataMigration {
    pub id: i64,
    pub tenant_id: String,
    pub source_target_key: String,
    pub target_key: String,
    pub source_target_mode: String,
    pub source_target_kind: String,
    pub target_target_mode: String,
    pub target_target_kind: String,
    pub source_generation: i64,
    pub source_switch_token: String,
    pub target_generation: i64,
    pub source_schema_fingerprint: String,
    pub target_schema_fingerprint: String,
    pub plan_hash: String,
    pub create_idempotency_key_hash: String,
    pub switch_token: String,
    pub operator_id: i64,
    pub retention_hours: i32,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct RegisterTenantDataBackupPoint {
    pub id: i64,
    pub scope: String,
    pub tenant_id: Option<String>,
    pub target_key: String,
    pub placement_generation: Option<i64>,
    pub schema_fingerprint: String,
    pub provider_ref: String,
    pub captured_at: DateTime<Utc>,
    pub checksum: Option<String>,
    pub validation_status: String,
    pub retention_until: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<i64>,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct ValidatedTenantDataBackup<'a> {
    pub tenant_id: &'a str,
    pub target_key: &'a str,
    pub target_mode: &'a str,
    pub target_generation: i64,
    pub schema_fingerprint: &'a str,
    pub not_before: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TenantDataRepository;

impl TenantDataRepository {
    /// 只读控制库批量快照，供目标列表计算 dedicated 资格；不连接任何目标库。
    pub async fn occupied_target_keys<C>(
        &self,
        db: &C,
        configured_target_keys: &[String],
    ) -> AppResult<HashSet<String>>
    where
        C: ConnectionTrait,
    {
        let configured_target_keys = configured_target_keys
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        if configured_target_keys.is_empty() {
            return Ok(HashSet::new());
        }
        if configured_target_keys.len() > 200 {
            return Err(AppError::Config(
                "tenant-data target occupancy snapshot exceeds the configured 200-target limit"
                    .into(),
            ));
        }
        let configured_target_keys = configured_target_keys.into_iter().collect::<Vec<_>>();
        let result_limit = configured_target_keys.len() as u64;
        let placements = tenant_data_placement::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_placement::Column::CurrentTargetKey)
            .filter(
                tenant_data_placement::Column::CurrentTargetKey
                    .is_in(configured_target_keys.clone()),
            )
            .filter(tenant_data_placement::Column::State.is_in([
                tenant_data_placement::Model::STATE_ACTIVE,
                tenant_data_placement::Model::STATE_MAINTENANCE,
                tenant_data_placement::Model::STATE_PROVISIONING,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        let prepared = tenant_data_migration::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_migration::Column::TargetKey)
            .filter(tenant_data_migration::Column::TargetKey.is_in(configured_target_keys.clone()))
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        let retained_sources = tenant_data_migration::Entity::find()
            .select_only()
            .distinct()
            .column(tenant_data_migration::Column::SourceTargetKey)
            .filter(
                tenant_data_migration::Column::SourceTargetKey
                    .is_in(configured_target_keys.clone()),
            )
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .limit(result_limit)
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(placements
            .into_iter()
            .chain(prepared)
            .chain(retained_sources)
            .collect())
    }

    pub async fn placement<C>(
        &self,
        db: &C,
        tenant_id: &str,
    ) -> AppResult<Option<tenant_data_placement::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_placement::Entity::find_by_id(tenant_id.to_owned())
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_placement_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<tenant_data_placement::Model> {
        tenant_data_placement::Entity::find_by_id(tenant_id.to_owned())
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户数据 placement 不存在".into()))
    }

    pub async fn save_placement_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        placement: tenant_data_placement::Model,
    ) -> AppResult<tenant_data_placement::Model> {
        tenant_data_placement::ActiveModel::from(placement)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn migration<C>(
        &self,
        db: &C,
        id: i64,
    ) -> AppResult<Option<tenant_data_migration::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn migrations_for_tenant<C>(
        &self,
        db: &C,
        tenant_id: &str,
        limit: u64,
    ) -> AppResult<Vec<tenant_data_migration::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration::Entity::find()
            .filter(tenant_data_migration::Column::TenantId.eq(tenant_id))
            .order_by_desc(tenant_data_migration::Column::CreatedAt)
            .order_by_desc(tenant_data_migration::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)
    }

    /// Worker watchdog 的 MySQL 权威扫描：返回必须拥有可恢复任务的迁移。
    pub async fn recoverable_migrations<C>(
        &self,
        db: &C,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<tenant_data_migration::Model>>
    where
        C: ConnectionTrait,
    {
        let mut query = tenant_data_migration::Entity::find().filter(
            Condition::any()
                .add(tenant_data_migration::Column::State.is_not_in([
                    tenant_data_migration::Model::STATE_FINALIZED,
                    tenant_data_migration::Model::STATE_FAILED,
                    tenant_data_migration::Model::STATE_CANCELLED,
                    tenant_data_migration::Model::STATE_RETENTION_PENDING,
                ]))
                .add(
                    Condition::all()
                        .add(
                            tenant_data_migration::Column::State
                                .eq(tenant_data_migration::Model::STATE_RETENTION_PENDING),
                        )
                        .add(tenant_data_migration::Column::FinalizeRequestedAt.is_not_null()),
                ),
        );
        if let Some(after_id) = after_id {
            query = query.filter(tenant_data_migration::Column::Id.gt(after_id));
        }
        query
            .order_by_asc(tenant_data_migration::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_migration_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<tenant_data_migration::Model> {
        tenant_data_migration::Entity::find_by_id(id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户数据迁移不存在".into()))
    }

    pub async fn migration_by_create_key<C>(
        &self,
        db: &C,
        key_hash: &str,
    ) -> AppResult<Option<tenant_data_migration::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration::Entity::find()
            .filter(tenant_data_migration::Column::CreateIdempotencyKeyHash.eq(key_hash))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn active_migration_for_tenant<C>(
        &self,
        db: &C,
        tenant_id: &str,
    ) -> AppResult<Option<tenant_data_migration::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration::Entity::find()
            .filter(tenant_data_migration::Column::TenantId.eq(tenant_id))
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .order_by_desc(tenant_data_migration::Column::CreatedAt)
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_active_migration_for_tenant_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<Option<tenant_data_migration::Model>> {
        tenant_data_migration::Entity::find()
            .filter(tenant_data_migration::Column::TenantId.eq(tenant_id))
            .filter(tenant_data_migration::Column::State.is_not_in([
                tenant_data_migration::Model::STATE_FINALIZED,
                tenant_data_migration::Model::STATE_FAILED,
                tenant_data_migration::Model::STATE_CANCELLED,
            ]))
            .order_by_desc(tenant_data_migration::Column::CreatedAt)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn insert_migration_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        command: CreateTenantDataMigration,
    ) -> AppResult<tenant_data_migration::Model> {
        tenant_data_migration::ActiveModel {
            id: Set(command.id),
            tenant_id: Set(command.tenant_id),
            source_target_key: Set(command.source_target_key),
            target_key: Set(command.target_key),
            source_target_mode: Set(command.source_target_mode),
            source_target_kind: Set(command.source_target_kind),
            target_target_mode: Set(command.target_target_mode),
            target_target_kind: Set(command.target_target_kind),
            source_generation: Set(command.source_generation),
            source_switch_token: Set(command.source_switch_token),
            target_generation: Set(command.target_generation),
            source_schema_fingerprint: Set(command.source_schema_fingerprint),
            target_schema_fingerprint: Set(command.target_schema_fingerprint),
            plan_hash: Set(command.plan_hash),
            create_idempotency_key_hash: Set(command.create_idempotency_key_hash),
            cancel_idempotency_key_hash: Set(None),
            finalize_idempotency_key_hash: Set(None),
            state: Set(tenant_data_migration::Model::STATE_PRECHECKING.into()),
            switch_token: Set(command.switch_token),
            operator_id: Set(command.operator_id),
            cancelled_by: Set(None),
            finalized_by: Set(None),
            background_job_id: Set(None),
            retention_hours: Set(command.retention_hours),
            error_code: Set(None),
            error_detail: Set(None),
            prechecked_at: Set(Some(command.now)),
            queued_at: Set(None),
            quiesced_at: Set(None),
            frozen_at: Set(None),
            copy_started_at: Set(None),
            copy_completed_at: Set(None),
            verified_at: Set(None),
            cut_over_at: Set(None),
            activated_at: Set(None),
            succeeded_at: Set(None),
            retention_until: Set(None),
            cancel_requested_at: Set(None),
            finalize_requested_at: Set(None),
            cleanup_ready_at: Set(None),
            finalized_at: Set(None),
            failed_at: Set(None),
            cancelled_at: Set(None),
            created_at: Set(command.now),
            updated_at: Set(command.now),
        }
        .insert(transaction)
        .await
        .map_err(database_error)
    }

    pub async fn save_migration_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        migration: tenant_data_migration::Model,
    ) -> AppResult<tenant_data_migration::Model> {
        tenant_data_migration::ActiveModel::from(migration)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn items<C>(
        &self,
        db: &C,
        migration_id: i64,
    ) -> AppResult<Vec<tenant_data_migration_item::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration_item::Entity::find()
            .filter(tenant_data_migration_item::Column::MigrationId.eq(migration_id))
            .order_by_asc(tenant_data_migration_item::Column::CopyOrder)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn insert_item<C>(
        &self,
        db: &C,
        item: tenant_data_migration_item::Model,
    ) -> AppResult<tenant_data_migration_item::Model>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration_item::ActiveModel::from(item)
            .insert(db)
            .await
            .map_err(database_error)
    }

    pub async fn save_item<C>(
        &self,
        db: &C,
        item: tenant_data_migration_item::Model,
    ) -> AppResult<tenant_data_migration_item::Model>
    where
        C: ConnectionTrait,
    {
        tenant_data_migration_item::ActiveModel::from(item)
            .reset_all()
            .update(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_item_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<tenant_data_migration_item::Model> {
        tenant_data_migration_item::Entity::find_by_id(id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("迁移表级检查点不存在".into()))
    }

    pub async fn backup_points_for_target<C>(
        &self,
        db: &C,
        target_key: &str,
        tenant_id: Option<&str>,
        limit: u64,
    ) -> AppResult<Vec<tenant_data_backup_point::Model>>
    where
        C: ConnectionTrait,
    {
        let mut query = tenant_data_backup_point::Entity::find()
            .filter(tenant_data_backup_point::Column::TargetKey.eq(target_key))
            .order_by_desc(tenant_data_backup_point::Column::CapturedAt);
        if let Some(tenant_id) = tenant_id {
            query = query.filter(
                Condition::any()
                    .add(
                        tenant_data_backup_point::Column::Scope
                            .eq(tenant_data_backup_point::Model::SCOPE_SHARD),
                    )
                    .add(
                        Condition::all()
                            .add(
                                tenant_data_backup_point::Column::Scope
                                    .eq(tenant_data_backup_point::Model::SCOPE_TENANT),
                            )
                            .add(tenant_data_backup_point::Column::TenantId.eq(tenant_id)),
                    ),
            );
        }
        query.limit(limit).all(db).await.map_err(database_error)
    }

    pub async fn backup_by_provider_ref<C>(
        &self,
        db: &C,
        provider_ref: &str,
    ) -> AppResult<Option<tenant_data_backup_point::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_data_backup_point::Entity::find()
            .filter(tenant_data_backup_point::Column::ProviderRef.eq(provider_ref))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn insert_backup<C>(
        &self,
        db: &C,
        command: RegisterTenantDataBackupPoint,
    ) -> AppResult<tenant_data_backup_point::Model>
    where
        C: ConnectionTrait,
    {
        tenant_data_backup_point::ActiveModel {
            id: Set(command.id),
            scope: Set(command.scope),
            tenant_id: Set(command.tenant_id),
            target_key: Set(command.target_key),
            placement_generation: Set(command.placement_generation),
            schema_fingerprint: Set(command.schema_fingerprint),
            provider_ref: Set(command.provider_ref),
            captured_at: Set(command.captured_at),
            checksum: Set(command.checksum),
            validation_status: Set(command.validation_status),
            validation_detail: Set(None),
            retention_until: Set(command.retention_until),
            expires_at: Set(command.expires_at),
            last_restore_drill_at: Set(None),
            created_by: Set(command.created_by),
            created_at: Set(command.now),
            updated_at: Set(command.now),
        }
        .insert(db)
        .await
        .map_err(database_error)
    }

    pub async fn validated_backup_for_destination<C>(
        &self,
        db: &C,
        query: ValidatedTenantDataBackup<'_>,
    ) -> AppResult<Option<tenant_data_backup_point::Model>>
    where
        C: ConnectionTrait,
    {
        let scope = if query.target_mode == "shared" {
            Condition::all()
                .add(
                    tenant_data_backup_point::Column::Scope
                        .eq(tenant_data_backup_point::Model::SCOPE_SHARD),
                )
                .add(tenant_data_backup_point::Column::TenantId.is_null())
                .add(tenant_data_backup_point::Column::PlacementGeneration.is_null())
        } else {
            Condition::all()
                .add(
                    tenant_data_backup_point::Column::Scope
                        .eq(tenant_data_backup_point::Model::SCOPE_TENANT),
                )
                .add(tenant_data_backup_point::Column::TenantId.eq(query.tenant_id))
                .add(
                    tenant_data_backup_point::Column::PlacementGeneration
                        .eq(query.target_generation),
                )
        };

        tenant_data_backup_point::Entity::find()
            .filter(tenant_data_backup_point::Column::TargetKey.eq(query.target_key))
            .filter(
                tenant_data_backup_point::Column::SchemaFingerprint.eq(query.schema_fingerprint),
            )
            .filter(
                tenant_data_backup_point::Column::ValidationStatus
                    .eq(tenant_data_backup_point::Model::VALIDATION_VALID),
            )
            .filter(tenant_data_backup_point::Column::Checksum.is_not_null())
            .filter(tenant_data_backup_point::Column::Checksum.ne(""))
            .filter(tenant_data_backup_point::Column::CapturedAt.gte(query.not_before))
            .filter(tenant_data_backup_point::Column::RetentionUntil.gte(query.now))
            .filter(
                Condition::any()
                    .add(tenant_data_backup_point::Column::ExpiresAt.is_null())
                    .add(tenant_data_backup_point::Column::ExpiresAt.gt(query.now)),
            )
            .filter(scope)
            .order_by_desc(tenant_data_backup_point::Column::CapturedAt)
            .one(db)
            .await
            .map_err(database_error)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
