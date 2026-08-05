use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, ExprTrait, QueryFilter, sea_query::Expr,
};

use crate::entities::cache_namespace_version;

/// 参数配置使用的规范缓存命名空间。
pub const CONFIG_CACHE_NAMESPACE: &str = "config";

/// 数据库权威缓存命名空间版本仓储。
pub struct CacheNamespaceVersionRepository;

impl CacheNamespaceVersionRepository {
    /// 读取权威版本；缺少行表示数据库初始化不完整，不能由 Redis 猜测默认值。
    pub async fn find_version(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        namespace: &str,
    ) -> AppResult<i64> {
        validate_cache_namespace(namespace)?;
        let row = cache_namespace_version::Entity::find_by_id((
            tenant_id.to_owned(),
            namespace.to_owned(),
        ))
        .one(db)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        match row {
            Some(row) if row.version >= 0 => Ok(row.version),
            Some(_) => Err(AppError::Database(format!(
                "租户 {tenant_id} 的缓存命名空间 {namespace} 权威版本无效"
            ))),
            None => Err(AppError::Database(format!(
                "租户 {tenant_id} 的缓存命名空间 {namespace} 缺少权威版本"
            ))),
        }
    }

    /// 在业务事务内原子递增版本，并返回该事务持有行锁时读到的新值。
    pub async fn increment_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        namespace: &str,
    ) -> AppResult<i64> {
        validate_cache_namespace(namespace)?;
        let result = cache_namespace_version::Entity::update_many()
            .col_expr(
                cache_namespace_version::Column::Version,
                Expr::col(cache_namespace_version::Column::Version).add(1),
            )
            .col_expr(
                cache_namespace_version::Column::UpdatedAt,
                Expr::value(chrono::Utc::now()),
            )
            .filter(cache_namespace_version::Column::TenantId.eq(tenant_id))
            .filter(cache_namespace_version::Column::Namespace.eq(namespace))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::Database(format!(
                "租户 {tenant_id} 的缓存命名空间 {namespace} 缺少权威版本"
            )));
        }

        cache_namespace_version::Entity::find_by_id((tenant_id.to_owned(), namespace.to_owned()))
            .one(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .map(|row| row.version)
            .filter(|version| *version >= 0)
            .ok_or_else(|| AppError::Database("缓存命名空间版本无效或已溢出".into()))
    }

    /// 为新租户创建初始版本。调用方必须与租户及模板数据在同一事务中提交。
    pub async fn insert_initial_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        namespace: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<()> {
        validate_cache_namespace(namespace)?;
        cache_namespace_version::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id.to_owned()),
            namespace: ActiveValue::Set(namespace.to_owned()),
            version: ActiveValue::Set(0),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(transaction)
        .await
        .map(|_| ())
        .map_err(|error| AppError::Database(error.to_string()))
    }
}

/// 校验 Redis 键、数据库主键与 Outbox 负载共享的规范命名空间。
pub fn validate_cache_namespace(namespace: &str) -> AppResult<()> {
    if namespace.is_empty()
        || namespace.len() > 64
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(AppError::Validation(
            "缓存命名空间只能包含 1 到 64 个小写字母、数字、点、下划线或连字符".into(),
        ));
    }
    Ok(())
}
