use async_trait::async_trait;
use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult, DataScope, DataScopeContext};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select,
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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))
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
            .map_err(|error| AppError::Database(error.to_string()))?;
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
        let select = Self::apply_filters(Self::base_select(tenant_id), filter);
        let Some(select) = Self::apply_data_scope(select, tenant_id, scope_ctx) else {
            return Ok(PageResult::new(vec![], 0, query));
        };

        crate::pagination::paginate(db, select.order_by_desc(user::Column::CreatedAt), query).await
    }

    /// 按主键递增游标读取数据范围内的用户，用于长时间运行的导出任务。
    pub async fn find_for_export_after_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        filter: &UserFilter<'_>,
        scope_ctx: &DataScopeContext,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<user::Model>> {
        let select = Self::apply_filters(Self::base_select(tenant_id), filter);
        let Some(mut select) = Self::apply_data_scope(select, tenant_id, scope_ctx) else {
            return Ok(Vec::new());
        };
        if let Some(after_id) = after_id {
            select = select.filter(user::Column::Id.gt(after_id));
        }
        select
            .order_by_asc(user::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
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

    /// 在一次当前读中解析数据范围访问权限并锁定目标用户。
    ///
    /// 安全敏感的用户变更必须在检查角色归属前调用此方法。用户行是与角色替换共享的
    /// 串行化点，因此等待者会观察到锁持有者已提交的角色。
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

    /// 在事务中锁定租户内用户，用于和文件引用计数保持一致的资料更新。
    pub async fn find_by_id_for_update(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<user::Model>> {
        Self::base_select(tenant_id)
            .filter(user::Column::Id.eq(id))
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在已锁定的用户行上更新头像 URL 和关联的文件元数据 ID。
    pub async fn update_avatar_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        avatar_url: String,
        avatar_file_id: i64,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<()> {
        let result = user::Entity::update_many()
            .col_expr(user::Column::Avatar, Expr::value(Some(avatar_url)))
            .col_expr(
                user::Column::AvatarFileId,
                Expr::value(Some(avatar_file_id)),
            )
            .col_expr(user::Column::UpdatedAt, Expr::value(updated_at))
            .filter(user::Column::Id.eq(id))
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::DelFlag.eq(user::Model::DEL_FLAG_NORMAL))
            .exec(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("用户不存在".into()));
        }
        Ok(())
    }

    /// 统计仍引用指定头像文件的有效用户数。
    pub async fn count_avatar_file_references_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        avatar_file_id: i64,
    ) -> AppResult<u64> {
        Self::base_select(tenant_id)
            .filter(user::Column::AvatarFileId.eq(avatar_file_id))
            .count(txn)
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
                Expr::value(user::Model::DEL_FLAG_DELETED),
            )
            .col_expr(user::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
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
            .col_expr(user::Column::Status, Expr::value(status))
            .col_expr(user::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
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

    pub async fn increment_authorization_versions<C>(
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
                user::Column::AuthorizationVersion,
                Expr::col(user::Column::AuthorizationVersion).add(1),
            )
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .filter(user::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|result| result.rows_affected)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在同一连接上读取指定用户的授权版本，供事务提交前生成镜像修复事件。
    pub async fn find_authorization_versions<C>(
        &self,
        db: &C,
        tenant_id: &str,
        user_ids: &[i64],
    ) -> AppResult<Vec<(i64, i32)>>
    where
        C: ConnectionTrait,
    {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut versions = user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .into_iter()
            .map(|user| (user.id, user.authorization_version))
            .collect::<Vec<_>>();
        versions.sort_unstable_by_key(|(user_id, _)| *user_id);
        Ok(versions)
    }
}
