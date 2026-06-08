use crate::{naming, schema::TableInfo};

pub fn render_repository(table: &TableInfo, _module: &str) -> String {
    let struct_name = naming::to_pascal_case(&table.table_name);
    let snake = naming::to_snake_case(&table.table_name);
    let pk_type = crate::schema::get_pk_type(table);

    format!(
        r#"use async_trait::async_trait;
use ryframe_common::{{AppError, AppResult}};
use ryframe_core::auto_fill::{{AutoFill, FillContext}};
use ryframe_core::repository::{{PageQuery, PageResult, Repository}};
use sea_orm::{{ColumnTrait, DatabaseConnection, EntityTrait, QueryOrder}};

use crate::entities::{snake};

pub struct {struct_name}Repository;

#[async_trait]
impl Repository<{snake}::Model, {pk_type}> for {struct_name}Repository {{
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: {pk_type},
    ) -> AppResult<Option<{snake}::Model>> {{
        {snake}::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }}

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<{snake}::Model>> {{
        ryframe_db::pagination::paginate(
            db,
            {snake}::Entity::find(),
            &query,
        )
        .await
    }}

    async fn insert(
        &self,
        db: &DatabaseConnection,
        mut entity: {snake}::Model,
    ) -> AppResult<{snake}::Model> {{
        entity.fill_on_insert(&FillContext::new());
        let active: {snake}::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }}

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: {snake}::Model,
    ) -> AppResult<{snake}::Model> {{
        let active: {snake}::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }}

    async fn delete(&self, db: &DatabaseConnection, id: {pk_type}) -> AppResult<()> {{
        {snake}::Entity::delete_by_id(id)
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }}
}}
"#,
        struct_name = struct_name,
        snake = snake,
        pk_type = pk_type,
    )
}
