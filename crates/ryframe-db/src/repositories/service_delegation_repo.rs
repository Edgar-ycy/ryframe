use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::LockType,
};

use crate::entities::{service_delegation, service_delegation_capability};

pub struct ServiceDelegationRepository;

impl ServiceDelegationRepository {
    pub async fn find_by_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        delegation_id: i64,
    ) -> AppResult<Option<service_delegation::Model>>
    where
        C: ConnectionTrait,
    {
        service_delegation::Entity::find_by_id(delegation_id)
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_for_user(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<Vec<service_delegation::Model>> {
        service_delegation::Entity::find()
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .filter(service_delegation::Column::UserId.eq(user_id))
            .order_by_desc(service_delegation::Column::CreatedAt)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_idempotent<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_id: i64,
        idempotency_key_hash: &[u8],
    ) -> AppResult<Option<service_delegation::Model>>
    where
        C: ConnectionTrait,
    {
        service_delegation::Entity::find()
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .filter(service_delegation::Column::UserId.eq(user_id))
            .filter(service_delegation::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 委托令牌没有 selector，因此用有限 Keyring 计算出的 MAC 候选进行等值定位。
    pub async fn find_by_mac_candidates(
        &self,
        db: &DatabaseConnection,
        token_mac_candidates: &[Vec<u8>],
    ) -> AppResult<Option<service_delegation::Model>> {
        if token_mac_candidates.is_empty() {
            return Ok(None);
        }
        service_delegation::Entity::find()
            .filter(
                service_delegation::Column::TokenMac.is_in(token_mac_candidates.iter().cloned()),
            )
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_id_for_share(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        delegation_id: i64,
    ) -> AppResult<Option<service_delegation::Model>> {
        service_delegation::Entity::find_by_id(delegation_id)
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .lock(LockType::Share)
            .one(txn)
            .await
            .map_err(database_error)
    }

    pub async fn insert_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        user_id: i64,
        entity: service_delegation::Model,
    ) -> AppResult<service_delegation::Model> {
        if entity.tenant_id != tenant_id
            || entity.user_id != user_id
            || entity.created_by_user_id != user_id
        {
            return Err(AppError::Authorization("委托只能由用户本人创建".into()));
        }
        service_delegation::ActiveModel::from(entity)
            .insert(txn)
            .await
            .map_err(database_error)
    }

    pub async fn replace_capabilities_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        delegation_id: i64,
        capability_keys: &[String],
    ) -> AppResult<()> {
        let mut normalized = capability_keys.to_vec();
        normalized.sort();
        normalized.dedup();
        service_delegation_capability::Entity::delete_many()
            .filter(service_delegation_capability::Column::TenantId.eq(tenant_id))
            .filter(service_delegation_capability::Column::DelegationId.eq(delegation_id))
            .exec(txn)
            .await
            .map_err(database_error)?;
        if !normalized.is_empty() {
            let models = normalized.into_iter().map(|capability_key| {
                service_delegation_capability::ActiveModel {
                    tenant_id: sea_orm::ActiveValue::Set(tenant_id.to_owned()),
                    delegation_id: sea_orm::ActiveValue::Set(delegation_id),
                    capability_key: sea_orm::ActiveValue::Set(capability_key),
                }
            });
            service_delegation_capability::Entity::insert_many(models)
                .exec(txn)
                .await
                .map_err(database_error)?;
        }
        Ok(())
    }

    pub async fn capability_keys<C>(
        &self,
        db: &C,
        tenant_id: &str,
        delegation_id: i64,
    ) -> AppResult<Vec<String>>
    where
        C: ConnectionTrait,
    {
        service_delegation_capability::Entity::find()
            .filter(service_delegation_capability::Column::TenantId.eq(tenant_id))
            .filter(service_delegation_capability::Column::DelegationId.eq(delegation_id))
            .order_by_asc(service_delegation_capability::Column::CapabilityKey)
            .all(db)
            .await
            .map(|rows| rows.into_iter().map(|row| row.capability_key).collect())
            .map_err(database_error)
    }

    /// 在 Agent 授权事务中按稳定键顺序取得委托能力共享锁。
    pub async fn capability_keys_for_share(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        delegation_id: i64,
    ) -> AppResult<Vec<String>> {
        service_delegation_capability::Entity::find()
            .filter(service_delegation_capability::Column::TenantId.eq(tenant_id))
            .filter(service_delegation_capability::Column::DelegationId.eq(delegation_id))
            .order_by_asc(service_delegation_capability::Column::CapabilityKey)
            .lock(LockType::Share)
            .all(txn)
            .await
            .map(|rows| rows.into_iter().map(|row| row.capability_key).collect())
            .map_err(database_error)
    }

    pub async fn revoke_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        delegation_id: i64,
        revoked_by: i64,
    ) -> AppResult<bool> {
        let result = service_delegation::Entity::update_many()
            .col_expr(
                service_delegation::Column::Status,
                sea_orm::sea_query::Expr::value(service_delegation::Model::STATUS_REVOKED),
            )
            .col_expr(
                service_delegation::Column::Version,
                sea_orm::sea_query::Expr::col(service_delegation::Column::Version).add(1),
            )
            .col_expr(
                service_delegation::Column::RevokedAt,
                sea_orm::sea_query::Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .col_expr(
                service_delegation::Column::RevokedBy,
                sea_orm::sea_query::Expr::value(revoked_by),
            )
            .col_expr(
                service_delegation::Column::UpdatedAt,
                sea_orm::sea_query::Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .filter(service_delegation::Column::Id.eq(delegation_id))
            .filter(service_delegation::Column::Status.eq(service_delegation::Model::STATUS_ACTIVE))
            .exec(txn)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    pub async fn count_active_at<C>(
        &self,
        db: &C,
        tenant_id: &str,
        account_id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        service_delegation::Entity::find()
            .filter(service_delegation::Column::TenantId.eq(tenant_id))
            .filter(service_delegation::Column::AccountId.eq(account_id))
            .filter(service_delegation::Column::UserId.eq(user_id))
            .filter(service_delegation::Column::Status.eq(service_delegation::Model::STATUS_ACTIVE))
            .filter(service_delegation::Column::NotBefore.lte(now))
            .filter(service_delegation::Column::ExpiresAt.gt(now))
            .count(db)
            .await
            .map_err(database_error)
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
