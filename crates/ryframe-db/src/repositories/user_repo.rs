use crate::DbResultExt;
use async_trait::async_trait;
use ryframe_kernel::{
    AppError, AppResult, DataScope, DataScopeContext, PageResult, ValidatedPageQuery,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select,
    sea_query::{Expr, LockType},
};

use crate::{Repository, entities::user};

mod mutations;

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
            .db()
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
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

    /// 仅用于在进入租户范围查询前区分跨租户访问和当前租户内不存在。
    pub async fn find_tenant_id_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<String>> {
        user::Entity::find_by_id(id)
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map(|user| user.map(|user| user.tenant_id))
            .db()
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

    fn export_select(
        tenant_id: &str,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> Option<Select<user::Entity>> {
        Self::apply_data_scope(
            Self::apply_filters(Self::base_select(tenant_id), filter),
            tenant_id,
            scope_ctx,
        )
    }

    /// 按当前数据范围读取租户内用户选择器结果，额外一条记录由调用方判断是否还有更多。
    pub async fn find_options_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: Option<&str>,
        scope_ctx: &DataScopeContext,
        limit: u64,
    ) -> AppResult<Vec<user::Model>> {
        let Some(mut select) =
            Self::apply_data_scope(Self::base_select(tenant_id), tenant_id, scope_ctx)
        else {
            return Ok(Vec::new());
        };
        if let Some(query) = query {
            select = select.filter(
                Condition::any()
                    .add(user::Column::Username.like(super::prefix_like(query)))
                    .add(user::Column::Nickname.like(super::prefix_like(query))),
            );
        }
        select
            .order_by_asc(user::Column::Username)
            .order_by_asc(user::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .db()
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
            .db()
    }

    pub async fn find_existing_usernames_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        usernames: &[String],
    ) -> AppResult<Vec<String>> {
        if usernames.is_empty() {
            return Ok(Vec::new());
        }
        user::Entity::find()
            .select_only()
            .column(user::Column::Username)
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::Username.is_in(usernames.iter().cloned()))
            .into_tuple::<String>()
            .all(transaction)
            .await
            .db()
    }

    /// 批量读取租户内仍有效用户的账号名称，供历史记录展示申请人而不暴露数据库 ID。
    pub async fn find_usernames_by_ids<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<Vec<(i64, String)>>
    where
        C: ConnectionTrait,
    {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        user::Entity::find()
            .select_only()
            .columns([user::Column::Id, user::Column::Username])
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .into_tuple::<(i64, String)>()
            .all(db)
            .await
            .db()
    }

    pub async fn insert_many_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        users: Vec<user::Model>,
    ) -> AppResult<()> {
        if users.is_empty() {
            return Ok(());
        }
        if users.iter().any(|user| user.tenant_id != tenant_id) {
            return Err(AppError::Authorization("批量用户租户不匹配".into()));
        }
        user::Entity::insert_many(users.into_iter().map(user::ActiveModel::from))
            .exec(transaction)
            .await
            .db()?;
        Ok(())
    }

    pub async fn find_by_page_filtered_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<user::Model>> {
        let Some(select) = Self::export_select(tenant_id, filter, scope_ctx) else {
            return Ok(PageResult::new(vec![], 0, query));
        };

        let select = select
            .order_by_desc(user::Column::CreatedAt)
            .order_by_desc(user::Column::Id);
        crate::pagination::paginate(db, select, query).await
    }

    /// 按主键递增游标读取数据范围内的用户，用于长时间运行的导出任务。
    pub async fn find_for_export_after_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
        window: ryframe_kernel::ExportCursorWindow,
    ) -> AppResult<Vec<user::Model>>
    where
        C: ConnectionTrait,
    {
        let Some(mut select) = Self::export_select(tenant_id, filter, scope_ctx) else {
            return Ok(Vec::new());
        };
        select = select.filter(user::Column::Id.lte(window.upper_id()));
        if let Some(after_id) = window.after_id() {
            select = select.filter(user::Column::Id.gt(after_id));
        }
        select
            .order_by_asc(user::Column::Id)
            .limit(window.limit())
            .all(db)
            .await
            .db()
    }

    /// 在同一主库快照内统计导出匹配行并捕获最大主键。
    pub async fn summarize_export<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<ryframe_kernel::ExportQuerySnapshot>
    where
        C: ConnectionTrait,
    {
        let Some(select) = Self::export_select(tenant_id, filter, scope_ctx) else {
            return Ok(ryframe_kernel::ExportQuerySnapshot {
                matched_rows: 0,
                upper_id: None,
            });
        };
        super::summarize_export_query(select, user::Column::Id, db).await
    }

    pub async fn find_by_page_with_data_scope(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
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
}
