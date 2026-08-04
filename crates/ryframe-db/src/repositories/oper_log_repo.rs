use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_core::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult, DataScopeContext};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect,
};

use crate::entities::oper_log;

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
    /// 在 Outbox 消费事务中按事件标识幂等写入操作日志。
    ///
    /// 返回 `true` 表示本次新增，返回 `false` 表示同一事件已经成功落库。
    pub async fn insert_event_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: oper_log::Model,
    ) -> AppResult<bool> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
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
    pub async fn find_for_export_after_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        filter: OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<oper_log::Model>> {
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
        if let Some(id) = after_id {
            select = select.filter(oper_log::Column::Id.gt(id));
        }
        select
            .order_by_asc(oper_log::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<oper_log::Model>> {
        let mut select = oper_log::Entity::find().filter(oper_log::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.oper_name.filter(|n| !n.is_empty()) {
            select = select.filter(oper_log::Column::OperName.contains(name));
        }
        if let Some(s) = filter.status.filter(|s| !s.is_empty()) {
            select = select.filter(oper_log::Column::Status.eq(s));
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
        select = select.order_by_desc(oper_log::Column::OperTime);
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

#[cfg(test)]
mod tests {
    use super::is_duplicate_key_error;

    #[test]
    fn only_unique_constraint_violations_are_idempotent_replays() {
        let duplicate = sea_orm::DbErr::Custom("ERROR 1062 duplicate entry".into());
        let unrelated = sea_orm::DbErr::Custom("connection closed".into());

        assert!(is_duplicate_key_error(&duplicate));
        assert!(!is_duplicate_key_error(&unrelated));
    }
}
