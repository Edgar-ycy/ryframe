use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult, DataScopeContext};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select,
};

use crate::entities::login_info;

pub struct LoginInfoRepository;

pub struct LoginInfoFilter<'a> {
    pub user_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

#[async_trait]
impl Repository<login_info::Model, i64> for LoginInfoRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<login_info::Model>> {
        login_info::Entity::find_by_id(id)
            .filter(login_info::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<login_info::Model>> {
        crate::pagination::paginate(
            db,
            login_info::Entity::find()
                .filter(login_info::Column::TenantId.eq(tenant_id))
                .order_by_desc(login_info::Column::LoginTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        insert_entity!(login_info, db, tenant_id, entity)
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _tenant_id: &str,
        _entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        Err(AppError::Internal("登录日志不支持修改".into()))
    }

    async fn delete(&self, _db: &DatabaseConnection, _tenant_id: &str, _id: i64) -> AppResult<()> {
        Err(AppError::Internal("登录日志不支持单条删除".into()))
    }
}

impl LoginInfoRepository {
    fn filtered_select(
        tenant_id: &str,
        filter: &LoginInfoFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> Select<login_info::Entity> {
        let mut select =
            login_info::Entity::find().filter(login_info::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.user_name.filter(|value| !value.is_empty()) {
            select = select.filter(login_info::Column::UserName.contains(name));
        }
        if let Some(status) = filter.status.filter(|value| !value.is_empty()) {
            select = select.filter(login_info::Column::Status.eq(status));
        }
        if let Some(begin) = filter.begin_time {
            select = select.filter(login_info::Column::LoginTime.gte(begin));
        }
        if let Some(end) = filter.end_time {
            select = select.filter(login_info::Column::LoginTime.lte(end));
        }
        if let Some(condition) = crate::data_scope::owner_username_condition(
            login_info::Column::UserName,
            tenant_id,
            scope_ctx,
        ) {
            select = select.filter(condition);
        }
        select
    }

    /// 按主键递增游标读取登录日志导出批次，并保留数据范围约束。
    pub async fn find_for_export_after_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &LoginInfoFilter<'_>,
        scope_ctx: &DataScopeContext,
        window: super::ExportCursorWindow,
    ) -> AppResult<Vec<login_info::Model>>
    where
        C: ConnectionTrait,
    {
        let mut select = Self::filtered_select(tenant_id, filter, scope_ctx)
            .filter(login_info::Column::Id.lte(window.upper_id()));
        if let Some(id) = window.after_id() {
            select = select.filter(login_info::Column::Id.gt(id));
        }
        select
            .order_by_asc(login_info::Column::Id)
            .limit(window.limit())
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在同一主库快照内统计导出匹配行并捕获最大主键。
    pub async fn summarize_export<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &LoginInfoFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<super::ExportQuerySnapshot>
    where
        C: ConnectionTrait,
    {
        super::summarize_export_query(
            Self::filtered_select(tenant_id, filter, scope_ctx),
            login_info::Column::Id,
            db,
        )
        .await
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: LoginInfoFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<login_info::Model>> {
        let select = Self::filtered_select(tenant_id, &filter, scope_ctx)
            .order_by_desc(login_info::Column::LoginTime)
            .order_by_desc(login_info::Column::Id);
        crate::pagination::paginate(db, select, query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection, tenant_id: &str) -> AppResult<u64> {
        login_info::Entity::delete_many()
            .filter(login_info::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn clean_all_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<u64> {
        login_info::Entity::delete_many()
            .filter(login_info::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map(|result| result.rows_affected)
            .map_err(|error| AppError::Database(error.to_string()))
    }
}
