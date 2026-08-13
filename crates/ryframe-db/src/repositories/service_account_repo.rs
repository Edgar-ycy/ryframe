use async_trait::async_trait;
use ryframe_core::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
};

use crate::entities::{role, service_account, service_account_role, tenant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceAccountLock {
    Share,
    Update,
}

impl ServiceAccountLock {
    const fn as_lock_type(self) -> LockType {
        match self {
            Self::Share => LockType::Share,
            Self::Update => LockType::Update,
        }
    }
}

pub struct ServiceAccountRepository;

#[async_trait]
impl Repository<service_account::Model, i64> for ServiceAccountRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<service_account::Model>> {
        base_select(tenant_id)
            .filter(service_account::Column::Id.eq(id))
            .one(db)
            .await
            .map_err(database_error)
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<service_account::Model>> {
        crate::pagination::paginate(
            db,
            base_select(tenant_id).order_by_desc(service_account::Column::CreatedAt),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: service_account::Model,
    ) -> AppResult<service_account::Model> {
        insert_entity!(service_account, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: service_account::Model,
    ) -> AppResult<service_account::Model> {
        update_entity!(service_account, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(service_account, db, tenant_id, id)
    }
}

impl ServiceAccountRepository {
    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<service_account::Model>> {
        base_select(tenant_id)
            .filter(service_account::Column::Code.eq(code))
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 授权栅栏要求事务先锁定租户，再按账号 ID 锁定服务账号。
    pub async fn lock_tenant_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        lock: ServiceAccountLock,
    ) -> AppResult<tenant::Model> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .lock(lock.as_lock_type())
            .one(txn)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    pub async fn find_by_id_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        lock: ServiceAccountLock,
    ) -> AppResult<Option<service_account::Model>> {
        base_select(tenant_id)
            .filter(service_account::Column::Id.eq(account_id))
            .lock(lock.as_lock_type())
            .one(txn)
            .await
            .map_err(database_error)
    }

    pub async fn insert_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        entity: service_account::Model,
    ) -> AppResult<service_account::Model> {
        insert_entity!(service_account, txn, tenant_id, entity)
    }

    pub async fn update_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        entity: service_account::Model,
    ) -> AppResult<service_account::Model> {
        update_entity!(service_account, txn, tenant_id, entity)
    }

    pub async fn soft_delete_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<()> {
        soft_delete_entity!(service_account, txn, tenant_id, account_id)
    }

    pub async fn increment_authorization_version_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
    ) -> AppResult<i32> {
        let result = service_account::Entity::update_many()
            .col_expr(
                service_account::Column::AuthorizationVersion,
                Expr::col(service_account::Column::AuthorizationVersion).add(1),
            )
            .col_expr(
                service_account::Column::UpdatedAt,
                Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(service_account::Column::TenantId.eq(tenant_id))
            .filter(service_account::Column::Id.eq(account_id))
            .exec(txn)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("服务账号不存在".into()));
        }
        self.find_by_id_in_txn(txn, tenant_id, account_id, ServiceAccountLock::Share)
            .await?
            .map(|account| account.authorization_version)
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))
    }

    pub async fn role_ids<C>(&self, db: &C, tenant_id: &str, account_id: i64) -> AppResult<Vec<i64>>
    where
        C: ConnectionTrait,
    {
        service_account_role::Entity::find()
            .filter(service_account_role::Column::TenantId.eq(tenant_id))
            .filter(service_account_role::Column::AccountId.eq(account_id))
            .order_by_asc(service_account_role::Column::RoleId)
            .all(db)
            .await
            .map(|rows| rows.into_iter().map(|row| row.role_id).collect())
            .map_err(database_error)
    }

    /// 替换角色前再次拒绝超级角色，数据库复合外键同时阻止跨租户绑定。
    pub async fn replace_roles_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        account_id: i64,
        role_ids: &[i64],
    ) -> AppResult<()> {
        self.find_by_id_in_txn(txn, tenant_id, account_id, ServiceAccountLock::Update)
            .await?
            .ok_or_else(|| AppError::NotFound("服务账号不存在".into()))?;
        let mut normalized = role_ids.to_vec();
        normalized.sort_unstable();
        normalized.dedup();
        if !normalized.is_empty() {
            let roles = role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Id.is_in(normalized.iter().copied()))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .order_by_asc(role::Column::Id)
                .lock(LockType::Share)
                .all(txn)
                .await
                .map_err(database_error)?;
            if roles.len() != normalized.len() {
                return Err(AppError::Validation("角色不存在或不属于当前租户".into()));
            }
            if roles.iter().any(|role| role.is_super != 0) {
                return Err(AppError::Validation("服务账号不能绑定超级角色".into()));
            }
        }
        service_account_role::Entity::delete_many()
            .filter(service_account_role::Column::TenantId.eq(tenant_id))
            .filter(service_account_role::Column::AccountId.eq(account_id))
            .exec(txn)
            .await
            .map_err(database_error)?;
        if !normalized.is_empty() {
            let models = normalized
                .into_iter()
                .map(|role_id| service_account_role::ActiveModel {
                    tenant_id: sea_orm::ActiveValue::Set(tenant_id.to_owned()),
                    account_id: sea_orm::ActiveValue::Set(account_id),
                    role_id: sea_orm::ActiveValue::Set(role_id),
                });
            service_account_role::Entity::insert_many(models)
                .exec(txn)
                .await
                .map_err(database_error)?;
        }
        Ok(())
    }
}

fn base_select(tenant_id: &str) -> sea_orm::Select<service_account::Entity> {
    service_account::Entity::find()
        .filter(service_account::Column::TenantId.eq(tenant_id))
        .filter(service_account::Column::DelFlag.eq(service_account::Model::DEL_FLAG_NORMAL))
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
