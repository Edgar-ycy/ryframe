use ryframe_common::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DatabaseTransaction, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, sea_query::LockType,
};

use crate::entities::{role, sys_file, tenant, user};

pub struct TenantRepository;

impl TenantRepository {
    fn quota_file_condition() -> Condition {
        Condition::any()
            .add(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .add(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
    }

    pub async fn list_all(&self, db: &DatabaseConnection) -> AppResult<Vec<tenant::Model>> {
        tenant::Entity::find()
            .order_by_asc(tenant::Column::TenantId)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// Check the user quota while holding the tenant row lock.
    ///
    /// Callers must insert the user in the same transaction. Serializing quota
    /// checks on the tenant row prevents concurrent creates from both observing
    /// the same remaining slot.
    pub async fn ensure_user_quota_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<()> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let count = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let limit = u64::try_from(tenant.max_users).unwrap_or_default();
        if count >= limit {
            return Err(AppError::Validation("已达到租户最大用户数".into()));
        }
        Ok(())
    }

    /// Lock the tenant row for quota-sensitive work performed in `txn`.
    ///
    /// Every caller must keep the quota check and the corresponding insert in
    /// this same transaction so all resource reservations share one lock order.
    pub async fn lock_tenant_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<tenant::Model> {
        Self::locked_tenant_query(tenant_id)
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    fn locked_tenant_query(tenant_id: &str) -> sea_orm::Select<tenant::Entity> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
    }

    pub async fn ensure_role_quota_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<()> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let count = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let limit = u64::try_from(tenant.max_roles).unwrap_or_default();
        if count >= limit {
            return Err(AppError::Validation("已达到租户最大角色数".into()));
        }
        Ok(())
    }

    /// Lock a tenant and reject limits that are below its current persisted usage.
    pub async fn lock_and_validate_resource_limits_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        max_users: i32,
        max_roles: i32,
        max_storage_mb: i64,
    ) -> AppResult<tenant::Model> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let user_count = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if user_count > u64::try_from(max_users).unwrap_or_default() {
            return Err(AppError::Validation(format!(
                "用户额度不能低于当前用户数 {user_count}"
            )));
        }

        let role_count = role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if role_count > u64::try_from(max_roles).unwrap_or_default() {
            return Err(AppError::Validation(format!(
                "角色额度不能低于当前角色数 {role_count}"
            )));
        }

        let storage_bytes = sys_file::Entity::find()
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(Self::quota_file_condition())
            .all(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .fold(0_u64, |used, file| {
                used.saturating_add(u64::try_from(file.file_size).unwrap_or_default())
            });
        let storage_limit_bytes = u64::try_from(max_storage_mb)
            .unwrap_or_default()
            .saturating_mul(1024 * 1024);
        if storage_bytes > storage_limit_bytes {
            return Err(AppError::Validation(format!(
                "存储额度不能低于当前已用字节数 {storage_bytes}"
            )));
        }

        Ok(tenant)
    }

    /// Check storage capacity under the same tenant-row lock used by uploads.
    ///
    /// The caller must insert the corresponding `sys_file` row before committing
    /// `txn`, so concurrent uploads cannot reserve the same remaining bytes.
    pub async fn ensure_storage_quota_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        incoming_bytes: u64,
    ) -> AppResult<()> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let used = sys_file::Entity::find()
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(Self::quota_file_condition())
            .all(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .fold(0_u64, |used, file| {
                used.saturating_add(u64::try_from(file.file_size).unwrap_or_default())
            });
        let limit = u64::try_from(tenant.max_storage_mb)
            .unwrap_or_default()
            .saturating_mul(1024 * 1024);
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

#[cfg(test)]
mod tests {
    use sea_orm::{DbBackend, QueryTrait};

    use super::TenantRepository;

    #[test]
    fn user_quota_query_locks_tenant_row() {
        let statement = TenantRepository::locked_tenant_query("tenant-a").build(DbBackend::MySql);

        assert!(statement.sql.ends_with("FOR UPDATE"));
        assert!(statement.sql.contains("`tenant_id` = ?"));
    }
}
