use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, ExprTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
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

    /// 持有租户行锁时检查用户配额。
    ///
    /// 调用方必须在同一事务中插入用户。将配额检查串行化到租户行上，可防止并发创建
    /// 同时观察到同一个剩余名额。
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

    /// 在租户行锁下为一整批新用户预留配额。
    pub async fn ensure_user_quota_for_batch_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        incoming: usize,
    ) -> AppResult<()> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let count = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let incoming =
            u64::try_from(incoming).map_err(|_| AppError::Validation("导入批次过大".into()))?;
        let limit = u64::try_from(tenant.max_users).unwrap_or_default();
        if count.saturating_add(incoming) > limit {
            return Err(AppError::Validation("批量导入将超过租户最大用户数".into()));
        }
        Ok(())
    }

    /// 为在 `txn` 中执行的配额敏感操作锁定租户行。
    ///
    /// 每个调用方都必须将配额检查及对应插入保留在同一事务中，使所有资源预留共享
    /// 同一种锁定顺序。
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

    /// 在调用方事务内递增租户授权纪元，并返回递增后的持久化值。
    pub async fn increment_authorization_epoch_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<i32> {
        self.lock_tenant_in_txn(txn, tenant_id).await?;
        let result = tenant::Entity::update_many()
            .col_expr(
                tenant::Column::AuthorizationEpoch,
                Expr::col(tenant::Column::AuthorizationEpoch).add(1),
            )
            .col_expr(tenant::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .exec(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .map(|tenant| tenant.authorization_epoch)
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

    /// 锁定租户，并拒绝低于其当前持久化用量的限制值。
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
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
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

    /// 在上传使用的同一租户行锁下检查存储容量。
    ///
    /// 调用方必须在提交 `txn` 前插入对应的 `sys_file` 记录，避免并发上传预留
    /// 同一批剩余字节。
    pub async fn ensure_storage_quota_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        incoming_bytes: u64,
    ) -> AppResult<()> {
        let tenant = self.lock_tenant_in_txn(txn, tenant_id).await?;
        let used = sys_file::Entity::find()
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
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
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        status: &str,
    ) -> AppResult<()> {
        let result = tenant::Entity::update_many()
            .col_expr(tenant::Column::Status, Expr::value(status))
            .col_expr(
                tenant::Column::SessionVersion,
                Expr::cust("session_version + 1"),
            )
            .col_expr(tenant::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        Ok(())
    }
}
