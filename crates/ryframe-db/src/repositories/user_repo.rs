use async_trait::async_trait;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect, Select,
    sea_query::{Expr, LockType},
};

use crate::entities::user;

pub struct UserRepository;

#[derive(Debug, Default)]
pub struct UserFilter<'a> {
    pub username: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub status: Option<&'a str>,
    pub dept_id: Option<i64>,
}

#[async_trait]
impl Repository<user::Model, i64> for UserRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<user::Model>> {
        Self::base_select(tenant_id)
            .filter(user::Column::Id.eq(id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
    ) -> AppResult<PageResult<user::Model>> {
        crate::pagination::paginate(
            db,
            Self::base_select(tenant_id).order_by_desc(user::Column::CreatedAt),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: user::Model,
    ) -> AppResult<user::Model> {
        insert_entity!(user, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: user::Model,
    ) -> AppResult<user::Model> {
        update_entity!(user, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(user, db, tenant_id, id)
    }
}

impl UserRepository {
    fn base_select(tenant_id: &str) -> Select<user::Entity> {
        user::Entity::find()
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::TenantId.eq(tenant_id))
    }

    fn apply_filters(
        mut select: Select<user::Entity>,
        filter: &UserFilter<'_>,
    ) -> Select<user::Entity> {
        if let Some(username) = filter.username.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Username.like(format!("%{}%", username)));
        }
        if let Some(phone) = filter.phone.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Phone.like(format!("%{}%", phone)));
        }
        if let Some(status) = filter.status.filter(|v| !v.is_empty()) {
            select = select.filter(user::Column::Status.eq(status));
        }
        if let Some(dept_id) = filter.dept_id {
            select = select.filter(user::Column::DeptId.eq(dept_id));
        }
        select
    }

    fn apply_data_scope(
        mut select: Select<user::Entity>,
        tenant_id: &str,
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
                let dept_id_text = dept_id.to_string();
                let descendant_condition = Condition::any()
                    .add(crate::entities::dept::Column::Ancestors.eq(&dept_id_text))
                    .add(
                        crate::entities::dept::Column::Ancestors
                            .like(format!("{},%", dept_id_text)),
                    )
                    .add(
                        crate::entities::dept::Column::Ancestors
                            .like(format!("%,{},%", dept_id_text)),
                    )
                    .add(
                        crate::entities::dept::Column::Ancestors
                            .like(format!("%,{}", dept_id_text)),
                    );
                select = select.filter(
                    Condition::any().add(user::Column::DeptId.eq(dept_id)).add(
                        user::Column::DeptId.in_subquery(
                            sea_orm::sea_query::Query::select()
                                .column(crate::entities::dept::Column::Id)
                                .from(crate::entities::dept::Entity)
                                .and_where(crate::entities::dept::Column::TenantId.eq(tenant_id))
                                .and_where(
                                    crate::entities::dept::Column::DelFlag
                                        .eq(crate::entities::dept::Model::DEL_FLAG_NORMAL),
                                )
                                .cond_where(descendant_condition)
                                .take(),
                        ),
                    ),
                );
            }
            DataScope::Custom => {
                if scope_ctx.custom_dept_ids.is_empty() && !scope_ctx.include_self {
                    return None;
                }
                let mut condition = Condition::any();
                if !scope_ctx.custom_dept_ids.is_empty() {
                    condition = condition
                        .add(user::Column::DeptId.is_in(scope_ctx.custom_dept_ids.clone()));
                }
                if scope_ctx.include_self {
                    condition = condition.add(user::Column::Id.eq(scope_ctx.user_id));
                }
                select = select.filter(condition);
            }
        }

        Some(select)
    }

    pub async fn find_by_username(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        username: &str,
    ) -> AppResult<Option<user::Model>> {
        Self::base_select(tenant_id)
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_page_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &PageQuery,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        let select = Self::apply_filters(Self::base_select(tenant_id), filter);
        let Some(select) = Self::apply_data_scope(select, tenant_id, scope_ctx) else {
            return Ok(PageResult::new(vec![], 0, query));
        };

        crate::pagination::paginate(db, select.order_by_desc(user::Column::CreatedAt), query).await
    }

    pub async fn find_by_page_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        self.find_by_page_filtered_with_data_scope(
            db,
            tenant_id,
            &query,
            &UserFilter::default(),
            scope_ctx,
        )
        .await
    }

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

    /// Resolve data-scope access and lock the target user as one current read.
    ///
    /// Security-sensitive user mutations must call this before inspecting role
    /// membership. The user row is the serialization point shared with role
    /// replacement, so a waiter observes roles committed by the lock holder.
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

    pub async fn delete_many<C>(&self, db: &C, tenant_id: &str, ids: &[i64]) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
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
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(result.rows_affected)
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
            .col_expr(
                user::Column::Status,
                sea_orm::sea_query::Expr::value(status),
            )
            .col_expr(
                user::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
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

    pub async fn increment_auth_versions<C>(
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
                user::Column::AuthVersion,
                Expr::col(user::Column::AuthVersion).add(1),
            )
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|result| result.rows_affected)
            .map_err(|error| AppError::Database(error.to_string()))
    }
}
