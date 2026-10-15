use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    AccessMode, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend,
    IsolationLevel, Statement, TransactionTrait,
};

use super::super::{BusinessMigration, BusinessMigrationOrigin, database_error};

/// 串行化整个账本的元数据登记，包括空账本的主键范围；取消时由事务回滚释放锁。
pub(in crate::migration) async fn begin_write(
    database: &DatabaseConnection,
) -> AppResult<DatabaseTransaction> {
    let transaction = database
        .begin_with_config(
            Some(IsolationLevel::Serializable),
            Some(AccessMode::ReadWrite),
        )
        .await
        .map_err(database_error)?;
    transaction.query_all_raw(Statement::from_string(DbBackend::MySql,
        format!("SELECT scope,module_name,migration_id FROM {} ORDER BY scope,module_name,migration_id FOR UPDATE", super::NAME)))
        .await.map_err(database_error)?;
    Ok(transaction)
}

/// 只持有明确表的元数据锁，不读取业务行，防止结构校验后、账本提交前发生 DDL。
pub(in crate::migration) async fn lock_business_tables<'a, C: ConnectionTrait>(
    database: &C,
    tables: impl IntoIterator<Item = &'a str>,
) -> AppResult<()> {
    let mut tables = tables.into_iter().collect::<Vec<_>>();
    tables.sort_unstable();
    tables.dedup();
    for table in tables {
        if table.is_empty()
            || table.len() > 64
            || !table
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(AppError::Config(
                "业务表名不是明确的安全标识符，拒绝锁定或写入".into(),
            ));
        }
        database
            .query_all_raw(Statement::from_string(
                DbBackend::MySql,
                format!("SELECT 1 FROM `{table}` LIMIT 0"),
            ))
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

pub(in crate::migration) async fn record_executed(
    database: &DatabaseConnection,
    migration: &dyn BusinessMigration,
) -> AppResult<()> {
    let transaction = begin_write(database).await?;
    let tables = super::table_names(migration);
    lock_business_tables(&transaction, tables.iter().map(String::as_str)).await?;
    super::verify_ownership(&transaction, migration).await?;
    migration.schema().verify_after(&transaction).await?;
    if let Some(state) = super::state(&transaction, migration).await? {
        state.verify(migration)?;
    } else {
        super::insert(
            &transaction,
            migration,
            BusinessMigrationOrigin::Executed,
            None,
        )
        .await?;
    }
    transaction.commit().await.map_err(database_error)
}
