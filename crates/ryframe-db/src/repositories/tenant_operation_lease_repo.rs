use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect,
    sea_query::{Condition, Expr, LockType},
};

use crate::entities::{
    tenant, tenant_config_transfer, tenant_data_migration, tenant_operation_lease,
};

/// 跨配置迁移、产品变更等操作共享的单租户租约仓储。
///
/// 所有方法固定按 `sys_tenant` → `sys_tenant_operation_lease` 的顺序加锁；调用方只能
/// 在此之后锁自己的资源行，避免不同操作形成环形等待。
pub struct TenantOperationLeaseRepository;

impl TenantOperationLeaseRepository {
    /// 锁定租户与统一租约，并验证当前调用者是否允许继续访问后续资源。
    pub async fn lock_tenant_and_validate_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: Option<&str>,
    ) -> AppResult<tenant::Model> {
        let tenant = tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let now = super::database_utc_now(transaction).await?;
        let lease = tenant_operation_lease::Entity::find_by_id(tenant_id.to_owned())
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?;
        match (owner_token, lease) {
            (Some(owner_token), Some(lease))
                if lease.expires_at > now && lease.owner_token == owner_token => {}
            (Some(_), _) => {
                return Err(AppError::TenantOperationConflict(
                    "租户操作租约已经过期、丢失或被其他执行者接管".into(),
                ));
            }
            (None, Some(lease)) if lease.expires_at <= now => {
                tenant_operation_lease::Entity::delete_by_id(tenant_id.to_owned())
                    .exec(transaction)
                    .await
                    .map_err(database_error)?;
            }
            (None, Some(_lease)) => {
                return Err(AppError::TenantOperationConflict(
                    "租户正在执行其他受控操作，请稍后重试".into(),
                ));
            }
            (None, None) => {}
        }
        Ok(tenant)
    }

    pub async fn acquire_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        lease: tenant_operation_lease::Model,
    ) -> AppResult<tenant_operation_lease::Model> {
        // 固定锁序的前两步：租户 → operation lease。资源行必须由调用方随后再锁。
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(&lease.tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let now = super::database_utc_now(transaction).await?;
        let existing = tenant_operation_lease::Entity::find_by_id(lease.tenant_id.clone())
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?;
        // TTL 只是崩溃检测机制，不是业务互斥边界。即使 lease 已过期，
        // control 库中未终态的迁移/配置操作仍保留所有权，只允许原资源恢复。
        let active_migration = tenant_data_migration::Entity::find()
            .filter(tenant_data_migration::Column::TenantId.eq(&lease.tenant_id))
            .filter(
                Condition::any()
                    .add(tenant_data_migration::Column::State.is_in([
                        tenant_data_migration::Model::STATE_PRECHECKING,
                        tenant_data_migration::Model::STATE_QUEUED,
                        tenant_data_migration::Model::STATE_QUIESCING,
                        tenant_data_migration::Model::STATE_FROZEN,
                        tenant_data_migration::Model::STATE_COPYING,
                        tenant_data_migration::Model::STATE_VERIFYING,
                        tenant_data_migration::Model::STATE_CUTTING_OVER,
                        tenant_data_migration::Model::STATE_ACTIVATING,
                        tenant_data_migration::Model::STATE_SUCCEEDED,
                    ]))
                    .add(
                        Condition::all()
                            .add(
                                tenant_data_migration::Column::State
                                    .eq(tenant_data_migration::Model::STATE_RETENTION_PENDING),
                            )
                            .add(tenant_data_migration::Column::FinalizeRequestedAt.is_not_null()),
                    ),
            )
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?;
        if active_migration
            .as_ref()
            .is_some_and(|migration| migration.switch_token != lease.owner_token)
        {
            return Err(AppError::TenantOperationConflict(
                "租户存在未终态数据迁移，不能被过期 lease 绕过".into(),
            ));
        }
        let active_config = tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(&lease.tenant_id))
            .filter(tenant_config_transfer::Column::Status.is_in([
                tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                tenant_config_transfer::Model::STATUS_APPLYING,
                tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                tenant_config_transfer::Model::STATUS_ROLLING_BACK,
            ]))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?;
        if active_config.as_ref().is_some_and(|transfer| {
            lease.resource_type != "tenant_config_transfer"
                || lease.resource_id.parse::<i64>().ok() != Some(transfer.id)
        }) {
            return Err(AppError::TenantOperationConflict(
                "租户存在未终态配置操作，不能被过期 lease 绕过".into(),
            ));
        }
        if let Some(existing) = existing {
            if existing.expires_at > now && existing.owner_token != lease.owner_token {
                return Err(AppError::TenantOperationConflict(
                    "租户正在执行其他受控操作，请稍后重试".into(),
                ));
            }
            let tenant_id = lease.tenant_id.clone();
            tenant_operation_lease::Entity::update_many()
                .col_expr(
                    tenant_operation_lease::Column::OwnerToken,
                    Expr::value(lease.owner_token),
                )
                .col_expr(
                    tenant_operation_lease::Column::Operation,
                    Expr::value(lease.operation),
                )
                .col_expr(
                    tenant_operation_lease::Column::ResourceType,
                    Expr::value(lease.resource_type),
                )
                .col_expr(
                    tenant_operation_lease::Column::ResourceId,
                    Expr::value(lease.resource_id),
                )
                .col_expr(
                    tenant_operation_lease::Column::ExpiresAt,
                    Expr::value(lease.expires_at),
                )
                .col_expr(
                    tenant_operation_lease::Column::UpdatedAt,
                    Expr::value(lease.updated_at),
                )
                .filter(tenant_operation_lease::Column::TenantId.eq(&tenant_id))
                .exec(transaction)
                .await
                .map_err(database_error)?;
            return tenant_operation_lease::Entity::find_by_id(tenant_id)
                .one(transaction)
                .await
                .map_err(database_error)?
                .ok_or_else(|| AppError::Conflict("租户操作租约写入失败".into()));
        }
        tenant_operation_lease::ActiveModel::from(lease)
            .insert(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn renew_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<bool> {
        self.lock_tenant_and_validate_in_txn(transaction, tenant_id, Some(owner_token))
            .await?;
        tenant_operation_lease::Entity::update_many()
            .col_expr(
                tenant_operation_lease::Column::ExpiresAt,
                Expr::value(expires_at),
            )
            .col_expr(
                tenant_operation_lease::Column::UpdatedAt,
                Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(tenant_operation_lease::Column::TenantId.eq(tenant_id))
            .filter(tenant_operation_lease::Column::OwnerToken.eq(owner_token))
            .exec(transaction)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }

    pub async fn release_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: &str,
    ) -> AppResult<bool> {
        self.lock_tenant_and_validate_in_txn(transaction, tenant_id, Some(owner_token))
            .await?;
        tenant_operation_lease::Entity::delete_many()
            .filter(tenant_operation_lease::Column::TenantId.eq(tenant_id))
            .filter(tenant_operation_lease::Column::OwnerToken.eq(owner_token))
            .exec(transaction)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
