use ryframe_common::{AppError, AppResult};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};

use crate::entities::{role, sys_file, tenant, user};

pub struct TenantRepository;

impl TenantRepository {
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
}
