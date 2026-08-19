use async_trait::async_trait;
use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, Statement,
};

use crate::entities::password_reset_request;

pub struct PasswordResetRequestRepository;

#[async_trait]
impl Repository<password_reset_request::Model, i64> for PasswordResetRequestRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<password_reset_request::Model>> {
        password_reset_request::Entity::find_by_id(id)
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<password_reset_request::Model>> {
        crate::pagination::paginate(
            db,
            password_reset_request::Entity::find()
                .filter(password_reset_request::Column::TenantId.eq(tenant_id))
                .order_by_desc(password_reset_request::Column::CreatedAt),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: password_reset_request::Model,
    ) -> AppResult<password_reset_request::Model> {
        insert_entity!(password_reset_request, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: password_reset_request::Model,
    ) -> AppResult<password_reset_request::Model> {
        update_entity!(password_reset_request, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        password_reset_request::Entity::delete_many()
            .filter(password_reset_request::Column::Id.eq(id))
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl PasswordResetRequestRepository {
    /// 读取 MySQL 的 UTC 时钟，确保各应用节点的过期和完成决策保持一致。
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<chrono::DateTime<chrono::Utc>>
    where
        C: ConnectionTrait + ?Sized,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::Database("database clock query returned no row".into()))?;
        let now: chrono::NaiveDateTime = row
            .try_get("", "db_now")
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(chrono::DateTime::from_naive_utc_and_offset(
            now,
            chrono::Utc,
        ))
    }

    /// 仅使 `evaluated_at` 时仍处于 pending 状态的请求过期。赢得行锁的并发完成操作
    /// 不能被持有过期前旧模型的调用方覆盖。
    pub async fn expire_pending(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        evaluated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        let result = Self::expire_pending_query(tenant_id, id, evaluated_at)
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    fn expire_pending_query(
        tenant_id: &str,
        id: i64,
        evaluated_at: chrono::DateTime<chrono::Utc>,
    ) -> sea_orm::UpdateMany<password_reset_request::Entity> {
        password_reset_request::Entity::update_many()
            .col_expr(
                password_reset_request::Column::Status,
                sea_orm::sea_query::Expr::value(password_reset_request::Model::STATUS_EXPIRED),
            )
            .col_expr(
                password_reset_request::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(evaluated_at),
            )
            .filter(password_reset_request::Column::Id.eq(id))
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .filter(
                password_reset_request::Column::Status
                    .eq(password_reset_request::Model::STATUS_PENDING),
            )
            .filter(password_reset_request::Column::CompletedAt.is_null())
            .filter(password_reset_request::Column::ExpiresAt.lte(evaluated_at))
    }

    /// 在调用方事务内原子消费仍有效的 pending 请求。
    ///
    /// 受守卫的更新是密码重置完成的串行化点：并发调用方可校验同一个令牌，但只有一个
    /// 调用方能将请求从 `pending` 转为 `completed`。
    pub async fn complete_pending_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        completed_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        let result = Self::complete_pending_query(tenant_id, id, completed_at)
            .exec(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    fn complete_pending_query(
        tenant_id: &str,
        id: i64,
        completed_at: chrono::DateTime<chrono::Utc>,
    ) -> sea_orm::UpdateMany<password_reset_request::Entity> {
        password_reset_request::Entity::update_many()
            .col_expr(
                password_reset_request::Column::Status,
                sea_orm::sea_query::Expr::value(password_reset_request::Model::STATUS_COMPLETED),
            )
            .col_expr(
                password_reset_request::Column::CompletedAt,
                sea_orm::sea_query::Expr::value(completed_at),
            )
            .col_expr(
                password_reset_request::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(completed_at),
            )
            .filter(password_reset_request::Column::Id.eq(id))
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .filter(
                password_reset_request::Column::Status
                    .eq(password_reset_request::Model::STATUS_PENDING),
            )
            .filter(password_reset_request::Column::CompletedAt.is_null())
            .filter(password_reset_request::Column::ExpiresAt.gt(completed_at))
    }

    pub async fn find_pending_by_target(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        target_user_id: i64,
    ) -> AppResult<Vec<password_reset_request::Model>> {
        password_reset_request::Entity::find()
            .filter(password_reset_request::Column::TenantId.eq(tenant_id))
            .filter(password_reset_request::Column::TargetUserId.eq(target_user_id))
            .filter(
                password_reset_request::Column::Status
                    .eq(password_reset_request::Model::STATUS_PENDING),
            )
            .order_by_desc(password_reset_request::Column::CreatedAt)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
