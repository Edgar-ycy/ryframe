use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};

use crate::entities::config;

pub struct ConfigRepository;

#[async_trait]
impl Repository<config::Model, i64> for ConfigRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<config::Model>> {
        config::Entity::find_by_id(id)
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<config::Model>> {
        crate::pagination::paginate(
            db,
            config::Entity::find()
                .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL)),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        let active: config::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        use sea_orm::sea_query::Expr;
        let now = chrono::Utc::now();
        // SeaORM 2.0-rc bug: ActiveModel::update() 在 auto_increment=false + MySQL
        // 时不发出 UPDATE，改用 update_many() 显式更新
        config::Entity::update_many()
            .col_expr(config::Column::Name, Expr::value(entity.name.clone()))
            .col_expr(config::Column::Key, Expr::value(entity.key.clone()))
            .col_expr(config::Column::Value, Expr::value(entity.value.clone()))
            .col_expr(config::Column::Remark, Expr::value(entity.remark.clone()))
            .col_expr(config::Column::UpdatedAt, Expr::value(now))
            .filter(config::Column::Id.eq(entity.id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        // 重新查询返回最新数据
        config::Entity::find_by_id(entity.id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = config::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(config::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl ConfigRepository {
    /// 按 key 查询配置
    pub async fn find_by_key(
        &self,
        db: &DatabaseConnection,
        key: &str,
    ) -> AppResult<Option<config::Model>> {
        config::Entity::find()
            .filter(config::Column::Key.eq(key))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
