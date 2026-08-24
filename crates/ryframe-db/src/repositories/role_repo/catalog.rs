use super::*;

impl RoleRepository {
    /// 按角色编码查找。
    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<role::Model>> {
        role::Entity::find()
            .filter(role::Column::Code.eq(code))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_super_role(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Option<role::Model>> {
        role::Entity::find()
            .filter(role::Column::IsSuper.eq(1))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        role_ids: &[i64],
    ) -> AppResult<Vec<role::Model>> {
        if role_ids.is_empty() {
            return Ok(Vec::new());
        }
        role::Entity::find()
            .filter(role::Column::Id.is_in(role_ids.to_vec()))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 查询角色关联的自定义数据权限部门 ID 列表。
    pub async fn find_role_dept_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        use crate::entities::role_dept;

        role_dept::Entity::find()
            .filter(role_dept::Column::RoleId.eq(role_id))
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map(|rows| rows.into_iter().map(|row| row.dept_id).collect())
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 查询多个角色的所有自定义部门 ID（合并去重）。
    pub async fn find_roles_dept_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        role_ids: &[i64],
    ) -> AppResult<Vec<i64>> {
        use crate::entities::role_dept;

        if role_ids.is_empty() {
            return Ok(vec![]);
        }
        let mut ids = role_dept::Entity::find()
            .filter(role_dept::Column::RoleId.is_in(role_ids.to_vec()))
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|row| row.dept_id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// 原子替换数据范围模式和自定义部门关系。
    pub async fn replace_data_scope(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        role_id: i64,
        data_scope: &str,
        dept_ids: &[i64],
    ) -> AppResult<()> {
        use sea_orm::ActiveValue;

        use crate::entities::role_dept;

        let updated = role::Entity::update_many()
            .col_expr(
                role::Column::DataScope,
                sea_orm::sea_query::Expr::value(data_scope),
            )
            .col_expr(
                role::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(role::Column::Id.eq(role_id))
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .exec(transaction)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        if updated.rows_affected != 1 {
            return Err(AppError::NotFound("角色不存在".into()));
        }

        role_dept::Entity::delete_many()
            .filter(role_dept::Column::RoleId.eq(role_id))
            .filter(role_dept::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        if !dept_ids.is_empty() {
            let relations = dept_ids.iter().map(|dept_id| role_dept::ActiveModel {
                tenant_id: ActiveValue::Set(tenant_id.to_owned()),
                role_id: ActiveValue::Set(role_id),
                dept_id: ActiveValue::Set(*dept_id),
            });
            role_dept::Entity::insert_many(relations)
                .exec(transaction)
                .await
                .map_err(|e| AppError::Database(e.to_string()))?;
        }
        Ok(())
    }
}
