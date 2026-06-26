macro_rules! insert_entity {
    ($entity:ident, $db:expr, $model:expr) => {{
        let active: $entity::ActiveModel = $model.into();
        active
            .insert($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }};
}

macro_rules! update_entity {
    ($entity:ident, $db:expr, $model:expr) => {{
        let tenant_id = ryframe_core::current_tenant_id();
        if $model.tenant_id != tenant_id {
            return Err(ryframe_common::AppError::Authorization(
                "不能修改其他租户的数据".to_string(),
            ));
        }
        let exists = $entity::Entity::find_by_id($model.id)
            .filter($entity::Column::TenantId.eq(&tenant_id))
            .one($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        if exists.is_none() {
            return Err(ryframe_common::AppError::NotFound("记录不存在".to_string()));
        }
        let active: $entity::ActiveModel = $model.into();
        active
            .update($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }};
}

macro_rules! soft_delete_entity {
    ($entity:ident, $db:expr, $id:expr) => {{
        let result = $entity::Entity::update_many()
            .col_expr(
                $entity::Column::DelFlag,
                sea_orm::sea_query::Expr::value($entity::Model::DEL_FLAG_DELETED.to_string()),
            )
            .col_expr(
                $entity::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter($entity::Column::Id.eq($id))
            .filter($entity::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .exec($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        if result.rows_affected == 0 {
            return Err(ryframe_common::AppError::NotFound("记录不存在".to_string()));
        }
        Ok(())
    }};
}
