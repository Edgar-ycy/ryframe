use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::prelude::*;

use super::tenant_data_catalog;
use super::{
    schema::{ensure_mysql, verify, verify_mysql_80},
    status::TENANT_DATA_MIGRATION_LEDGER,
};

const MIGRATION_LOCK_SQL_PREFIX: &str = "ryframe:tenant-data-migration:";

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        let mut migrations: Vec<Box<dyn MigrationTrait>> = vec![
            Box::new(super::m20260820_000000_tenant_baseline::Migration),
            Box::new(super::m20261017_000000_data_scope_projection::Migration),
        ];
        migrations.extend(crate::generated::migrations());
        migrations
    }

    fn migration_table_name() -> DynIden {
        Alias::new(TENANT_DATA_MIGRATION_LEDGER).into_iden()
    }
}

/// 升级一个明确选择的租户数据目标。不会创建任何控制面 `sys_*` 表。
pub async fn up(db: &DatabaseConnection) -> Result<(), DbErr> {
    up_pending_business_migrations(db).await?;
    verify(db).await
}

/// 应用框架租户库迁移但不立即校验业务表。
///
/// 组合根会先调用此入口，再运行已注册业务模块的租户迁移，最后统一校验完整 schema。
pub async fn up_pending_business_migrations(db: &DatabaseConnection) -> Result<(), DbErr> {
    ensure_mysql(db)?;
    verify_mysql_80(db).await?;
    tenant_data_catalog()
        .validate()
        .map_err(|error| DbErr::Custom(format!("tenant-data catalog is invalid: {error}")))?;

    let transaction = db.begin().await?;
    if let Err(error) = acquire_migration_lock(&transaction).await {
        let _ = transaction.rollback().await;
        return Err(error);
    }
    let migration_result = Migrator::up(&transaction, None)
        .await
        .map_err(|error| DbErr::Custom(format!("tenant-data migration failed: {error}")));
    let release_result = release_migration_lock(&transaction).await;
    match migration_result.and(release_result) {
        Ok(()) => transaction.commit().await?,
        Err(error) => {
            let _ = transaction.rollback().await;
            return Err(error);
        }
    }
    Ok(())
}

async fn acquire_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT GET_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256), 60)"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "timed out waiting for the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

/// 组合根持有目标迁移互斥时，在该会话执行框架迁移，业务迁移随后统一核验。
pub async fn up_in_session(session: &mut sea_orm::DatabaseTransaction) -> Result<(), DbErr> {
    tenant_data_catalog().validate().map_err(DbErr::Custom)?;
    let held = scalar_i64_on(session, "SELECT IS_USED_LOCK(SHA2(CONCAT('ryframe:tenant-data-migration:',DATABASE()),256))=CONNECTION_ID()".into()).await?;
    if held != 1 {
        return Err(DbErr::Custom("初始化会话未持有租户迁移互斥".into()));
    }
    Migrator::up(&*session, None).await?;
    session.finish_mysql_root(true).await
}

async fn release_migration_lock<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let value = scalar_i64_on(
        db,
        format!(
            "SELECT RELEASE_LOCK(SHA2(CONCAT('{MIGRATION_LOCK_SQL_PREFIX}', DATABASE()), 256))"
        ),
    )
    .await?;
    if value != 1 {
        return Err(DbErr::Custom(
            "failed to release the tenant-data migration lock".into(),
        ));
    }
    Ok(())
}

async fn scalar_i64_on<C>(db: &C, sql: String) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql))
        .await?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned no result".into()))?;
    Option::<i64>::try_get_by_index(&row, 0)?
        .ok_or_else(|| DbErr::Custom("tenant-data migration lock returned NULL".into()))
}
