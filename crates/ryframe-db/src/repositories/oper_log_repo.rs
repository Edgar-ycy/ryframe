use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult, DataScopeContext, PageResult, ValidatedPageQuery};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select,
};

use crate::{Repository, entities::oper_log};

pub struct OperLogRepository;

pub struct OperLogFilter<'a> {
    pub oper_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

#[async_trait]
impl Repository<oper_log::Model, i64> for OperLogRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<oper_log::Model>> {
        oper_log::Entity::find_by_id(id)
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<oper_log::Model>> {
        crate::pagination::paginate(
            db,
            oper_log::Entity::find()
                .filter(oper_log::Column::TenantId.eq(tenant_id))
                .order_by_desc(oper_log::Column::OperTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: oper_log::Model,
    ) -> AppResult<oper_log::Model> {
        insert_entity!(oper_log, db, tenant_id, entity)
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _tenant_id: &str,
        _entity: oper_log::Model,
    ) -> AppResult<oper_log::Model> {
        Err(AppError::Internal("操作日志不支持修改".into()))
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        oper_log::Entity::delete_many()
            .filter(oper_log::Column::Id.eq(id))
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl OperLogRepository {
    fn filtered_select(
        tenant_id: &str,
        filter: &OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> Select<oper_log::Entity> {
        let mut select = oper_log::Entity::find().filter(oper_log::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.oper_name.filter(|value| !value.is_empty()) {
            select = select.filter(oper_log::Column::OperName.contains(name));
        }
        if let Some(status) = filter.status.filter(|value| !value.is_empty()) {
            select = select.filter(oper_log::Column::Status.eq(status));
        }
        if let Some(begin) = filter.begin_time {
            select = select.filter(oper_log::Column::OperTime.gte(begin));
        }
        if let Some(end) = filter.end_time {
            select = select.filter(oper_log::Column::OperTime.lte(end));
        }
        if let Some(condition) = crate::data_scope::owner_username_condition(
            oper_log::Column::OperName,
            tenant_id,
            scope_ctx,
        ) {
            select = select.filter(condition);
        }
        select
    }

    /// 在 Outbox 消费事务中按事件标识幂等写入操作日志。
    ///
    /// 返回 `true` 表示本次新增，返回 `false` 表示同一事件已经成功落库。
    pub async fn insert_event_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: oper_log::Model,
    ) -> AppResult<bool> {
        ryframe_kernel::TenantId::parse(tenant_id)?;
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("不能新增其他租户的操作日志".into()));
        }
        let event_id = entity
            .event_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Validation("审计事件标识不能为空".into()))?
            .to_owned();

        match oper_log::ActiveModel::from(entity)
            .insert(transaction)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if is_duplicate_key_error(&error) => {
                let existing = oper_log::Entity::find()
                    .filter(oper_log::Column::EventId.eq(&event_id))
                    .one(transaction)
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| {
                        AppError::Database("操作日志唯一键冲突后未读取到审计事件".into())
                    })?;
                if existing.tenant_id != tenant_id {
                    return Err(AppError::Authorization(
                        "审计事件标识已经属于其他租户".into(),
                    ));
                }
                Ok(false)
            }
            Err(error) => Err(database_error(error)),
        }
    }

    /// 按主键递增游标读取操作日志导出批次，并保留数据范围约束。
    pub async fn find_for_export_after_id<C>(
        &self,
        db: &C,
        tenant_id: &str,
        filter: &OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
        window: ryframe_kernel::ExportCursorWindow,
    ) -> AppResult<Vec<oper_log::Model>>
    where
        C: ConnectionTrait,
    {
        let mut select = Self::filtered_select(tenant_id, filter, scope_ctx)
            .filter(oper_log::Column::Id.lte(window.upper_id()));
        if let Some(id) = window.after_id() {
            select = select.filter(oper_log::Column::Id.gt(id));
        }
        select
            .order_by_asc(oper_log::Column::Id)
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
        filter: &OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<ryframe_kernel::ExportQuerySnapshot>
    where
        C: ConnectionTrait,
    {
        super::summarize_export_query(
            Self::filtered_select(tenant_id, filter, scope_ctx),
            oper_log::Column::Id,
            db,
        )
        .await
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<oper_log::Model>> {
        let select = Self::filtered_select(tenant_id, &filter, scope_ctx)
            .order_by_desc(oper_log::Column::OperTime)
            .order_by_desc(oper_log::Column::Id);
        crate::pagination::paginate(db, select, query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection, tenant_id: &str) -> AppResult<u64> {
        oper_log::Entity::delete_many()
            .filter(oper_log::Column::TenantId.eq(tenant_id))
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
        oper_log::Entity::delete_many()
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map(|result| result.rows_affected)
            .map_err(|error| AppError::Database(error.to_string()))
    }
}

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("duplicate") || text.contains("1062")
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
