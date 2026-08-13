use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, sea_query::LockType,
};

use crate::entities::service_credential;

pub struct ServiceCredentialRepository;

impl ServiceCredentialRepository {
    /// 仅用于从公开 Key ID 定位租户和账号；验证 Secret 前不能信任该提示。
    pub async fn find_hint_by_key_id(
        &self,
        db: &DatabaseConnection,
        key_id: &str,
    ) -> AppResult<Option<service_credential::Model>> {
        service_credential::Entity::find()
            .filter(service_credential::Column::KeyId.eq(key_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        account_id: i64,
        credential_id: i64,
    ) -> AppResult<Option<service_credential::Model>>
    where
        C: ConnectionTrait,
    {
        service_credential::Entity::find_by_id(credential_id)
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_for_account<C>(
        &self,
        db: &C,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<Vec<service_credential::Model>>
    where
        C: ConnectionTrait,
    {
        service_credential::Entity::find()
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .order_by_desc(service_credential::Column::CreatedAt)
            .all(db)
            .await
            .map_err(database_error)
    }

    pub async fn count_active_at<C>(
        &self,
        db: &C,
        tenant_id: &str,
        account_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        service_credential::Entity::find()
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .filter(service_credential::Column::Status.eq(service_credential::Model::STATUS_ACTIVE))
            .filter(service_credential::Column::RevokedAt.is_null())
            .filter(service_credential::Column::ExpiresAt.gt(now))
            .count(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_idempotent<C>(
        &self,
        db: &C,
        tenant_id: &str,
        account_id: i64,
        idempotency_key_hash: &[u8],
    ) -> AppResult<Option<service_credential::Model>>
    where
        C: ConnectionTrait,
    {
        service_credential::Entity::find()
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .filter(service_credential::Column::IdempotencyKeyHash.eq(idempotency_key_hash))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_key_id_for_share(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        key_id: &str,
    ) -> AppResult<Option<service_credential::Model>> {
        service_credential::Entity::find()
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .filter(service_credential::Column::KeyId.eq(key_id))
            .lock(LockType::Share)
            .one(txn)
            .await
            .map_err(database_error)
    }

    pub async fn insert_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        entity: service_credential::Model,
    ) -> AppResult<service_credential::Model> {
        if entity.tenant_id != tenant_id || entity.account_id != account_id {
            return Err(AppError::Authorization("凭据租户或服务账号不匹配".into()));
        }
        service_credential::ActiveModel::from(entity)
            .insert(txn)
            .await
            .map_err(database_error)
    }

    pub async fn revoke_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        credential_id: i64,
        revoked_by: i64,
    ) -> AppResult<bool> {
        let result = service_credential::Entity::update_many()
            .col_expr(
                service_credential::Column::Status,
                sea_orm::sea_query::Expr::value(service_credential::Model::STATUS_REVOKED),
            )
            .col_expr(
                service_credential::Column::RevokedAt,
                sea_orm::sea_query::Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .col_expr(
                service_credential::Column::RevokedBy,
                sea_orm::sea_query::Expr::value(revoked_by),
            )
            .col_expr(
                service_credential::Column::UpdatedAt,
                sea_orm::sea_query::Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::AccountId.eq(account_id))
            .filter(service_credential::Column::Id.eq(credential_id))
            .filter(service_credential::Column::Status.eq(service_credential::Model::STATUS_ACTIVE))
            .exec(txn)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    pub async fn touch_last_used<C>(
        &self,
        db: &C,
        tenant_id: &str,
        credential_id: i64,
        at: DateTime<Utc>,
    ) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        service_credential::Entity::update_many()
            .col_expr(
                service_credential::Column::LastUsedAt,
                sea_orm::sea_query::Expr::value(at),
            )
            .filter(service_credential::Column::TenantId.eq(tenant_id))
            .filter(service_credential::Column::Id.eq(credential_id))
            .exec(db)
            .await
            .map(|_| ())
            .map_err(database_error)
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
