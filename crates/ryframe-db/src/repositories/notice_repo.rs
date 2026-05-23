use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::notice;

pub struct NoticeRepository;

#[async_trait]
impl Repository<notice::Model, i64> for NoticeRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<notice::Model>> {
        notice::Entity::find_by_id(id).filter(notice::Column::DelFlag.eq(notice::Model::DEL_FLAG_NORMAL)).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<notice::Model>> {
        crate::pagination::paginate(db, notice::Entity::find().filter(notice::Column::DelFlag.eq(notice::Model::DEL_FLAG_NORMAL)), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: notice::Model) -> AppResult<notice::Model> {
        let active: notice::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: notice::Model) -> AppResult<notice::Model> {
        let active: notice::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = notice::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(notice::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl NoticeRepository {
    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        title: Option<&str>,
        notice_type: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<notice::Model>> {
        let mut select = notice::Entity::find()
            .filter(notice::Column::DelFlag.eq(notice::Model::DEL_FLAG_NORMAL));
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            select = select.filter(notice::Column::Title.like(format!("%{}%", t)));
        }
        if let Some(nt) = notice_type.filter(|nt| !nt.is_empty()) {
            select = select.filter(notice::Column::Type.eq(nt));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(notice::Column::Status.eq(s));
        }
        select = select.order_by_desc(notice::Column::CreatedAt);
        crate::pagination::paginate(db, select, &query).await
    }
}
