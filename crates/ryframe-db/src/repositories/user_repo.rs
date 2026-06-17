use async_trait::async_trait;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Select,
};

use crate::entities::user;

pub struct UserRepository;
const DEFAULT_TENANT_ID: &str = "system";

#[async_trait]
impl Repository<user::Model, i64> for UserRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<user::Model>> {
        user::Entity::find_by_id(id)
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<user::Model>> {
        crate::pagination::paginate(
            db,
            Self::base_select().order_by_desc(user::Column::CreatedAt),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: user::Model) -> AppResult<user::Model> {
        insert_entity!(user, db, entity)
    }

    async fn update(&self, db: &DatabaseConnection, entity: user::Model) -> AppResult<user::Model> {
        update_entity!(user, db, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        soft_delete_entity!(user, db, id)
    }
}

impl UserRepository {
    fn base_select() -> Select<user::Entity> {
        user::Entity::find()
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::TenantId.eq(DEFAULT_TENANT_ID))
    }

    fn apply_filters(
        mut select: Select<user::Entity>,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
    ) -> Select<user::Entity> {
        if let Some(username) = username.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Username.like(format!("%{}%", username)));
        }
        if let Some(phone) = phone.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Phone.like(format!("%{}%", phone)));
        }
        if let Some(status) = status.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Status.eq(status));
        }
        if let Some(dept_id) = dept_id {
            select = select.filter(user::Column::DeptId.eq(dept_id));
        }
        select
    }

    fn apply_data_scope(
        mut select: Select<user::Entity>,
        scope_ctx: &DataScopeContext,
    ) -> Option<Select<user::Entity>> {
        match &scope_ctx.scope {
            DataScope::All => {}
            DataScope::SelfOnly => {
                select = select.filter(user::Column::Id.eq(scope_ctx.user_id));
            }
            DataScope::Dept => {
                let dept_id = scope_ctx.dept_id?;
                select = select.filter(user::Column::DeptId.eq(dept_id));
            }
            DataScope::DeptAndChildren => {
                let dept_id = scope_ctx.dept_id?;
                select = select.filter(
                    Condition::any().add(user::Column::DeptId.eq(dept_id)).add(
                        user::Column::DeptId.in_subquery(
                            sea_orm::sea_query::Query::select()
                                .column(crate::entities::dept::Column::Id)
                                .from(crate::entities::dept::Entity)
                                .and_where(
                                    crate::entities::dept::Column::Ancestors
                                        .like(format!("%{}%", dept_id)),
                                )
                                .take(),
                        ),
                    ),
                );
            }
            DataScope::Custom => {
                if scope_ctx.custom_dept_ids.is_empty() {
                    return None;
                }
                select =
                    select.filter(user::Column::DeptId.is_in(scope_ctx.custom_dept_ids.clone()));
            }
        }

        Some(select)
    }

    pub async fn find_by_username(
        &self,
        db: &DatabaseConnection,
        username: &str,
    ) -> AppResult<Option<user::Model>> {
        Self::base_select()
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
    ) -> AppResult<PageResult<user::Model>> {
        let select = Self::apply_filters(Self::base_select(), username, phone, status, dept_id)
            .order_by_desc(user::Column::CreatedAt);
        crate::pagination::paginate(db, select, &query).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn find_by_page_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        let select = Self::apply_filters(Self::base_select(), username, phone, status, dept_id);
        let Some(select) = Self::apply_data_scope(select, scope_ctx) else {
            return Ok(PageResult::new(vec![], 0, &query));
        };

        crate::pagination::paginate(db, select.order_by_desc(user::Column::CreatedAt), &query).await
    }

    pub async fn find_by_page_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        self.find_by_page_filtered_with_data_scope(db, query, None, None, None, None, scope_ctx)
            .await
    }

    pub async fn find_by_id_with_data_scope(
        &self,
        db: &DatabaseConnection,
        id: i64,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<Option<user::Model>> {
        let select = Self::base_select().filter(user::Column::Id.eq(id));
        let Some(select) = Self::apply_data_scope(select, scope_ctx) else {
            return Ok(None);
        };

        select
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn delete_many(&self, db: &DatabaseConnection, ids: &[i64]) -> AppResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = user::Entity::update_many()
            .col_expr(
                user::Column::DelFlag,
                sea_orm::sea_query::Expr::value(user::Model::DEL_FLAG_DELETED),
            )
            .col_expr(
                user::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(user::Column::Id.is_in(ids.to_vec()))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(result.rows_affected)
    }

    pub async fn update_status(
        &self,
        db: &DatabaseConnection,
        id: i64,
        status: String,
    ) -> AppResult<()> {
        let active = user::ActiveModel {
            id: ActiveValue::Unchanged(id),
            status: ActiveValue::Set(status),
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
