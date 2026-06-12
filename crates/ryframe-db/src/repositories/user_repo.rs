use async_trait::async_trait;
use ryframe_common::{
    AppError, AppResult,
    annotations::data_scope::{DataScope, DataScopeContext},
};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder,
};

use crate::entities::user;

pub struct UserRepository;

#[async_trait]
impl Repository<user::Model, i64> for UserRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<user::Model>> {
        user::Entity::find_by_id(id)
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<user::Model>> {
        crate::pagination::paginate(
            db,
            user::Entity::find().filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL)),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: user::Model) -> AppResult<user::Model> {
        let active: user::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: user::Model) -> AppResult<user::Model> {
        let active: user::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = user::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(user::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl UserRepository {
    /// 按用户名查找用户
    pub async fn find_by_username(
        &self,
        db: &DatabaseConnection,
        username: &str,
    ) -> AppResult<Option<user::Model>> {
        user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        username: Option<&str>,
        phone: Option<&str>,
        status: Option<&str>,
        dept_id: Option<i64>,
    ) -> AppResult<PageResult<user::Model>> {
        let mut select =
            user::Entity::find().filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL));

        if let Some(u) = username.filter(|u| !u.is_empty()) {
            select = select.filter(user::Column::Username.like(format!("%{}%", u)));
        }
        if let Some(p) = phone.filter(|p| !p.is_empty()) {
            select = select.filter(user::Column::Phone.like(format!("%{}%", p)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(user::Column::Status.eq(s));
        }
        if let Some(d) = dept_id {
            select = select.filter(user::Column::DeptId.eq(d));
        }

        select = select.order_by_desc(user::Column::CreatedAt);
        crate::pagination::paginate(db, select, &query).await
    }

    /// 批量删除用户
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

    /// 更新用户状态
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

    /// 带数据权限过滤的分页查询
    ///
    /// 根据 DataScopeContext 自动注入 WHERE 条件：
    /// - All: 不过滤
    /// - SelfOnly: 只看自己
    /// - Dept: 只看本部门
    /// - DeptAndChildren: 本部门及子部门
    /// - Custom: 自定义部门列表
    pub async fn find_by_page_with_data_scope(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        let mut select =
            user::Entity::find().filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL));

        match &scope_ctx.scope {
            DataScope::All => { /* 不过滤 */ }
            DataScope::SelfOnly => {
                select = select.filter(user::Column::Id.eq(scope_ctx.user_id));
            }
            DataScope::Dept => {
                match scope_ctx.dept_id {
                    Some(did) => {
                        select = select.filter(user::Column::DeptId.eq(did));
                    }
                    None => {
                        // 无部门，返回空
                        return Ok(PageResult::new(vec![], 0, &query));
                    }
                }
            }
            DataScope::DeptAndChildren => {
                match scope_ctx.dept_id {
                    Some(did) => {
                        // 子查询：找本部门 + 所有 ancestors 以本部门ID路径开头的子部门
                        select = select.filter(
                            Condition::any().add(user::Column::DeptId.eq(did)).add(
                                user::Column::DeptId.in_subquery(
                                    sea_orm::sea_query::Query::select()
                                        .column(crate::entities::dept::Column::Id)
                                        .from(crate::entities::dept::Entity)
                                        .and_where(
                                            crate::entities::dept::Column::Ancestors
                                                .like(format!("%{}%", did)),
                                        )
                                        .take(),
                                ),
                            ),
                        );
                    }
                    None => {
                        return Ok(PageResult::new(vec![], 0, &query));
                    }
                }
            }
            DataScope::Custom => {
                if scope_ctx.custom_dept_ids.is_empty() {
                    return Ok(PageResult::new(vec![], 0, &query));
                }
                select =
                    select.filter(user::Column::DeptId.is_in(scope_ctx.custom_dept_ids.clone()));
            }
        }

        crate::pagination::paginate(db, select, &query).await
    }
}
