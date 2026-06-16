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
        let active: $entity::ActiveModel = $model.into();
        active
            .update($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }};
}

macro_rules! soft_delete_entity {
    ($entity:ident, $db:expr, $id:expr) => {{
        let active = $entity::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged($id),
            del_flag: sea_orm::ActiveValue::Set($entity::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: sea_orm::ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update($db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }};
}
