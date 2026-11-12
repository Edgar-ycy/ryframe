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
    register_record(&transaction, migration).await?;
    transaction.commit().await.map_err(database_error)
}

/// 会话由组合根保留，DDL 后重新开始元数据登记事务，不嵌套租用连接。
pub(in crate::migration) async fn record_in_session(
    database: &mut DatabaseTransaction,
    migration: &dyn BusinessMigration,
) -> AppResult<()> {
    if database.is_open() {
        return Err(AppError::Config("业务登记不能结束调用方的活跃事务".into()));
    }
    database
        .begin_mysql_root(
            Some(IsolationLevel::Serializable),
            Some(AccessMode::ReadWrite),
        )
        .await
        .map_err(database_error)?;
    let result = async {
        database.query_all_raw(Statement::from_string(DbBackend::MySql,
            format!("SELECT scope,module_name,migration_id FROM {} ORDER BY scope,module_name,migration_id FOR UPDATE",super::NAME))).await.map_err(database_error)?;
        register_record(database,migration).await?;
        Ok::<(),AppError>(())
    }.await;
    if let Err(error) = result {
        let rollback = database.finish_mysql_root(false).await;
        return Err(AppError::Database(format!(
            "{error}；回滚结果={rollback:?}；先核对完整前后像，不自动重放"
        )));
    }
    database.finish_mysql_root(true).await.map_err(|error| {
        AppError::Database(format!(
            "业务账本提交结果未知。先核对完整前后像，不自动重放。底层错误 {error}"
        ))
    })
}
async fn register_record<C: ConnectionTrait>(
    database: &C,
    migration: &dyn BusinessMigration,
) -> AppResult<()> {
    let tables = super::table_names(migration);
    lock_business_tables(database, tables.iter().map(String::as_str)).await?;
    super::verify_ownership(database, migration).await?;
    migration.schema().verify_after(database).await?;
    if let Some(state) = super::state(database, migration).await? {
        state.verify(migration)?;
    } else {
        super::insert(database, migration, BusinessMigrationOrigin::Executed, None).await?;
    }

    Ok(())
}
