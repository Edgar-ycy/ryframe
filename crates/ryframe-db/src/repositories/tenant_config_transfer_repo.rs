use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
    sea_query::{Expr, LockType},
};

use crate::{
    entities::{
        tenant, tenant_config_bundle, tenant_config_transfer, tenant_config_transfer_item,
        tenant_operation_lease,
    },
    repositories::TenantOperationLeaseRepository,
};

/// 在租户行锁保护下读取的配置与授权版本。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantConfigurationFence {
    pub configuration_version: i64,
    pub authorization_epoch: i32,
}

pub struct TenantConfigTransferRepository;

impl TenantConfigTransferRepository {
    /// 使用数据库时钟作为租约、过期和计划版本的统一时间来源。
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
    where
        C: ConnectionTrait,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("数据库时钟查询没有返回记录".into()))?;
        let value: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
        Ok(DateTime::from_naive_utc_and_offset(value, Utc))
    }

    /// 锁定租户配置栅栏，并拒绝由其他所有者持有的有效配置迁移租约。
    ///
    /// 所有日常配置写入和迁移任务必须先调用本方法，再按稳定顺序锁定资源行。
    pub async fn lock_tenant_configuration_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: Option<&str>,
    ) -> AppResult<TenantConfigurationFence> {
        let tenant = TenantOperationLeaseRepository
            .lock_tenant_and_validate_in_txn(transaction, tenant_id, owner_token)
            .await?;
        Ok(TenantConfigurationFence {
            configuration_version: tenant.configuration_version,
            authorization_epoch: tenant.authorization_epoch,
        })
    }

    /// 在已持有租户行锁的事务中递增配置版本并返回新值。
    pub async fn increment_configuration_version_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i64> {
        let result = tenant::Entity::update_many()
            .col_expr(
                tenant::Column::ConfigurationVersion,
                Expr::col(tenant::Column::ConfigurationVersion).add(1),
            )
            .col_expr(tenant::Column::UpdatedAt, Expr::cust("UTC_TIMESTAMP(6)"))
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .one(transaction)
            .await
            .map_err(database_error)?
            .map(|tenant| tenant.configuration_version)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    pub async fn acquire_lease_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        lease: tenant_operation_lease::Model,
    ) -> AppResult<tenant_operation_lease::Model> {
        TenantOperationLeaseRepository
            .acquire_in_txn(transaction, lease)
            .await
    }

    pub async fn renew_lease_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<bool> {
        TenantOperationLeaseRepository
            .renew_in_txn(transaction, tenant_id, owner_token, expires_at)
            .await
    }

    pub async fn release_lease_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        owner_token: &str,
    ) -> AppResult<bool> {
        TenantOperationLeaseRepository
            .release_in_txn(transaction, tenant_id, owner_token)
            .await
    }

    pub async fn insert_bundle<C>(
        &self,
        db: &C,
        bundle: tenant_config_bundle::Model,
    ) -> AppResult<tenant_config_bundle::Model>
    where
        C: ConnectionTrait,
    {
        tenant_config_bundle::ActiveModel::from(bundle)
            .insert(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_bundle_by_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<tenant_config_bundle::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_config_bundle::Entity::find_by_id(id)
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_bundles_by_ids<C>(
        &self,
        db: &C,
        tenant_id: &str,
        ids: &[i64],
    ) -> AppResult<Vec<tenant_config_bundle::Model>>
    where
        C: ConnectionTrait,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_bundle::Column::Id.is_in(ids.iter().copied()))
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_bundles(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        limit: u64,
        offset: u64,
    ) -> AppResult<Vec<tenant_config_bundle::Model>> {
        tenant_config_bundle::Entity::find()
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .order_by_desc(tenant_config_bundle::Column::CreatedAt)
            .order_by_desc(tenant_config_bundle::Column::Id)
            .limit(limit.clamp(1, 100))
            .offset(offset)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_bundle_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<tenant_config_bundle::Model>> {
        tenant_config_bundle::Entity::find_by_id(id)
            .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn update_bundle<C>(
        &self,
        db: &C,
        bundle: tenant_config_bundle::Model,
    ) -> AppResult<tenant_config_bundle::Model>
    where
        C: ConnectionTrait,
    {
        tenant_config_bundle::ActiveModel::from(bundle)
            .reset_all()
            .update(db)
            .await
            .map_err(database_error)
    }

    pub async fn insert_transfer<C>(
        &self,
        db: &C,
        transfer: tenant_config_transfer::Model,
    ) -> AppResult<tenant_config_transfer::Model>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer::ActiveModel::from(transfer)
            .insert(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_transfer_by_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<tenant_config_transfer::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer::Entity::find_by_id(id)
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_transfer_by_idempotency_key<C>(
        &self,
        db: &C,
        tenant_id: &str,
        requested_by: i64,
        idempotency_key_hash: &str,
    ) -> AppResult<Option<tenant_config_transfer::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer::Column::RequestedBy.eq(requested_by))
            .filter(tenant_config_transfer::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_transfers(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        limit: u64,
        offset: u64,
    ) -> AppResult<Vec<tenant_config_transfer::Model>> {
        tenant_config_transfer::Entity::find()
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .order_by_desc(tenant_config_transfer::Column::CreatedAt)
            .order_by_desc(tenant_config_transfer::Column::Id)
            .limit(limit.clamp(1, 100))
            .offset(offset)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_transfer_by_background_job<C>(
        &self,
        db: &C,
        background_job_id: i64,
    ) -> AppResult<Option<tenant_config_transfer::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer::Entity::find()
            .filter(
                sea_orm::Condition::any()
                    .add(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(background_job_id))
                    .add(
                        tenant_config_transfer::Column::RollbackBackgroundJobId
                            .eq(background_job_id),
                    ),
            )
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_transfer_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<tenant_config_transfer::Model>> {
        tenant_config_transfer::Entity::find_by_id(id)
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn update_transfer<C>(
        &self,
        db: &C,
        transfer: tenant_config_transfer::Model,
    ) -> AppResult<tenant_config_transfer::Model>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer::ActiveModel::from(transfer)
            .reset_all()
            .update(db)
            .await
            .map_err(database_error)
    }

    /// 原子替换一次预览的全部项目；调用方应先锁迁移记录。
    pub async fn replace_items_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        transfer_id: i64,
        items: Vec<tenant_config_transfer_item::Model>,
    ) -> AppResult<()> {
        if items
            .iter()
            .any(|item| item.tenant_id != tenant_id || item.transfer_id != transfer_id)
        {
            return Err(AppError::Authorization("配置迁移明细租户不匹配".into()));
        }
        tenant_config_transfer_item::Entity::delete_many()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if !items.is_empty() {
            tenant_config_transfer_item::Entity::insert_many(
                items
                    .into_iter()
                    .map(tenant_config_transfer_item::ActiveModel::from),
            )
            .exec(transaction)
            .await
            .map_err(database_error)?;
        }
        Ok(())
    }

    pub async fn list_items<C>(
        &self,
        db: &C,
        tenant_id: &str,
        transfer_id: i64,
    ) -> AppResult<Vec<tenant_config_transfer_item::Model>>
    where
        C: ConnectionTrait,
    {
        tenant_config_transfer_item::Entity::find()
            .filter(tenant_config_transfer_item::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer_item::Column::TransferId.eq(transfer_id))
            .order_by_asc(tenant_config_transfer_item::Column::Id)
            .all(db)
            .await
            .map_err(database_error)
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
