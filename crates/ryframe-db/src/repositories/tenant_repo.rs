use ryframe_common::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
};

use crate::entities::{role, sys_file, tenant, user};

pub struct TenantRepository;

impl TenantRepository {
    pub async fn list_all(&self, db: &DatabaseConnection) -> AppResult<Vec<tenant::Model>> {
        tenant::Entity::find()
            .order_by_asc(tenant::Column::TenantId)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    async fn current(&self, db: &DatabaseConnection, tenant_id: &str) -> AppResult<tenant::Model> {
        self.find_by_tenant_id(db, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    pub async fn ensure_user_quota(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<()> {
        let tenant = self.current(db, tenant_id).await?;
        let count = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .count(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if count >= tenant.max_users as u64 {
            return Err(AppError::Validation("已达到租户最大用户数".into()));
        }
        Ok(())
    }

    pub async fn ensure_role_quota(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<tenant::Model> {
        let tenant = self.current(db, tenant_id).await?;
        let count = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .count(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if count >= tenant.max_roles as u64 {
            return Err(AppError::Validation("已达到租户最大角色数".into()));
        }
        Ok(tenant)
    }

    pub async fn ensure_storage_quota(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        incoming_bytes: u64,
    ) -> AppResult<()> {
        let tenant = self.current(db, tenant_id).await?;
        let used = sys_file::Entity::find()
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .map(|file| file.file_size.max(0) as u64)
            .sum::<u64>();
        let limit = tenant.max_storage_mb.max(0) as u64 * 1024 * 1024;
        if used.saturating_add(incoming_bytes) > limit {
            return Err(AppError::Validation("已达到租户最大存储容量".into()));
        }
        Ok(())
    }
    pub async fn find_by_tenant_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Option<tenant::Model>> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn ensure_available(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<tenant::Model> {
        let tenant = self
            .find_by_tenant_id(db, tenant_id)
            .await?
            .ok_or_else(|| AppError::Authentication("租户不存在".into()))?;
        if tenant.status != tenant::Model::STATUS_NORMAL {
            return Err(AppError::Authentication("租户已停用".into()));
        }
        if tenant
            .expire_at
            .is_some_and(|expire_at| expire_at <= chrono::Utc::now())
        {
            return Err(AppError::Authentication("租户已到期".into()));
        }
        Ok(tenant)
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        tenant: tenant::Model,
    ) -> AppResult<tenant::Model> {
        use sea_orm::sea_query::Expr;

        let tenant_id = tenant.tenant_id.clone();
        let result = tenant::Entity::update_many()
            .col_expr(tenant::Column::Name, Expr::value(tenant.name))
            .col_expr(tenant::Column::Domain, Expr::value(tenant.domain))
            .col_expr(tenant::Column::Status, Expr::value(tenant.status))
            .col_expr(tenant::Column::ExpireAt, Expr::value(tenant.expire_at))
            .col_expr(tenant::Column::MaxUsers, Expr::value(tenant.max_users))
            .col_expr(tenant::Column::MaxRoles, Expr::value(tenant.max_roles))
            .col_expr(
                tenant::Column::MaxStorageMb,
                Expr::value(tenant.max_storage_mb),
            )
            .col_expr(
                tenant::Column::MaxRequestsPerMin,
                Expr::value(tenant.max_requests_per_min),
            )
            .col_expr(
                tenant::Column::SessionVersion,
                Expr::value(tenant.session_version),
            )
            .col_expr(tenant::Column::UpdatedAt, Expr::value(tenant.updated_at))
            .filter(tenant::Column::Id.eq(tenant.id))
            .filter(tenant::Column::TenantId.eq(&tenant_id))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        self.find_by_tenant_id(db, &tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    pub async fn update_status(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        status: &str,
    ) -> AppResult<()> {
        let result = tenant::Entity::update_many()
            .col_expr(
                tenant::Column::Status,
                sea_orm::sea_query::Expr::value(status),
            )
            .col_expr(
                tenant::Column::SessionVersion,
                sea_orm::sea_query::Expr::cust("session_version + 1"),
            )
            .col_expr(
                tenant::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        Ok(())
    }
}
