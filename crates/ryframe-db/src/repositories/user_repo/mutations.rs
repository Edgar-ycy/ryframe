use super::*;

impl UserRepository {
    pub async fn find_by_id_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<Option<user::Model>> {
        let select = Self::base_select(tenant_id).filter(user::Column::Id.eq(id));
        let Some(select) = Self::apply_data_scope(select, tenant_id, scope_ctx) else {
            return Ok(None);
        };
        select
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 在一次当前读中解析数据范围访问权限并锁定目标用户。
    pub async fn find_by_id_with_data_scope_for_update(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<Option<user::Model>> {
        let select = Self::base_select(tenant_id).filter(user::Column::Id.eq(id));
        let Some(select) = Self::apply_data_scope(select, tenant_id, scope_ctx) else {
            return Ok(None);
        };
        select
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn find_by_id_for_update(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<user::Model>> {
        Self::base_select(tenant_id)
            .filter(user::Column::Id.eq(id))
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn update_avatar_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<()> {
        let result = user::Entity::update_many()
            .col_expr(user::Column::Avatar, Expr::value(Some(avatar_url)))
            .col_expr(
                user::Column::AvatarFileId,
                Expr::value(Some(avatar_file_id)),
            )
            .col_expr(user::Column::UpdatedAt, Expr::value(updated_at))
            .filter(user::Column::Id.eq(id))
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .exec(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        Ok(())
    }

    pub async fn count_avatar_file_references_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        avatar_file_id: i64,
    ) -> AppResult<u64> {
        Self::base_select(tenant_id)
            .filter(user::Column::AvatarFileId.eq(avatar_file_id))
            .count(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn delete_many<C>(&self, db: &C, tenant_id: &str, ids: &[i64]) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        if ids.is_empty() {
            return Ok(0);
        }
        user::Entity::update_many()
            .col_expr(
                user::Column::DelFlag,
                Expr::value(user::Model::DEL_FLAG_DELETED),
            )
            .col_expr(user::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(user::Column::Id.is_in(ids.to_vec()))
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|result| result.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn update_status<C>(
        &self,
        db: &C,
        tenant_id: &str,
        id: i64,
        status: String,
    ) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        let result = user::Entity::update_many()
            .col_expr(user::Column::Status, Expr::value(status))
            .col_expr(user::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(user::Column::Id.eq(id))
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        Ok(())
    }

    pub async fn increment_authorization_versions<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        if user_ids.is_empty() {
            return Ok(0);
        }
        user::Entity::update_many()
            .col_expr(
                user::Column::AuthorizationVersion,
                Expr::col(user::Column::AuthorizationVersion).add(1),
            )
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|result| result.rows_affected)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn find_authorization_versions<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<Vec<(i64, i32)>>
    where
        C: ConnectionTrait,
    {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut versions = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .map(|user| (user.id, user.authorization_version))
            .collect::<Vec<_>>();
        versions.sort_unstable_by_key(|(user_id, _)| *user_id);
        Ok(versions)
    }
}
