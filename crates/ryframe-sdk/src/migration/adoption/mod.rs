use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement};

use super::{
    BusinessMigration, BusinessMigrationOrigin, BusinessMigrationScope, database_error, ledger,
    schema,
};

mod model;
mod state;
pub use model::{
    BusinessAdoptionLedger, BusinessAdoptionManifest, BusinessAdoptionMigration,
    BusinessAdoptionOutcome, BusinessAdoptionPlan, BusinessAdoptionState, BusinessAdoptionTable,
    BusinessAdoptionTarget,
};

/// 只读生成明确模块的接管计划；不创建账本或修改业务表。
pub async fn preview_business_adoption(
    database: &DatabaseConnection,
    scope: BusinessMigrationScope,
    module: &str,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<BusinessAdoptionPlan> {
    let selected = state::selected(scope, module, migrations)?;
    schema::validate_business_migration_ledger(database, scope, migrations).await?;
    let target = state::identity(database).await?;
    state::verify_tables(database, &selected).await?;
    let snapshot = ledger::load_snapshot(database).await?;
    if snapshot
        .rows
        .iter()
        .any(|row| row.record.scope == scope.as_str() && row.record.module == module)
    {
        return Err(AppError::Config(
            "所选模块已全部或部分登记，不能重新接管；使用原计划只读核验或检查迁移状态".into(),
        ));
    }
    BusinessAdoptionPlan::new(BusinessAdoptionManifest {
        version: 1,
        scope,
        module: module.to_owned(),
        target,
        ledger: BusinessAdoptionLedger {
            existed: snapshot.exists,
            fingerprint: model::fingerprint(&snapshot.rows)?,
        },
        migrations: state::bindings(&selected),
        tables: state::table_bindings(&selected),
    })
}

/// 对照完整前后像只读判断；冲突、部分登记或未知来源作为错误返回。
pub async fn inspect_business_adoption(
    database: &DatabaseConnection,
    plan: &BusinessAdoptionPlan,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<BusinessAdoptionState> {
    let selected = state::validate_source(plan, migrations)?;
    schema::validate_business_migration_ledger(database, plan.manifest.scope, migrations).await?;
    state::verify_identity(database, plan).await?;
    state::verify_tables(database, &selected).await?;
    let snapshot = ledger::load_snapshot(database).await?;
    state::classify(plan, &selected, &snapshot, false)
}

/// 显式登记已审阅计划；只写框架元数据，不运行历史 DDL，不更改业务数据。
pub async fn apply_business_adoption(
    database: &DatabaseConnection,
    plan: &BusinessAdoptionPlan,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<BusinessAdoptionOutcome> {
    let selected = state::validate_source(plan, migrations)?;
    if inspect_business_adoption(database, plan, migrations).await?
        == BusinessAdoptionState::Applied
    {
        return Ok(BusinessAdoptionOutcome {
            already_applied: true,
            registered: 0,
            metadata_ddl_attempted: false,
        });
    }
    let metadata_ddl_attempted = !plan.manifest.ledger.existed;
    if metadata_ddl_attempted {
        database.execute_raw(Statement::from_string(DbBackend::MySql, ledger::DDL)).await
            .map_err(|error| AppError::Database(format!("接管元数据建表失败：{error}；可能已创建空账本，先核对现场，业务表未修改，不自动重放")))?;
    }
    let transaction = ledger::write::begin_write(database).await?;
    let prepared = prepare_locked(
        &transaction,
        plan,
        migrations,
        &selected,
        metadata_ddl_attempted,
    )
    .await;
    let status = match prepared {
        Ok(status) => status,
        Err(error) => return rollback_error(transaction, error, metadata_ddl_attempted).await,
    };
    if status == BusinessAdoptionState::Applied {
        transaction.rollback().await.map_err(database_error)?;
        return Ok(BusinessAdoptionOutcome {
            already_applied: true,
            registered: 0,
            metadata_ddl_attempted,
        });
    }
    let writes = write_records(&transaction, plan, &selected).await;
    if let Err(error) = writes {
        return rollback_error(transaction, error, metadata_ddl_attempted).await;
    }
    transaction.commit().await.map_err(|error| AppError::Database(format!("接管元数据提交结果未知：{error}；计划={}，业务表结构和数据未由本操作修改；先只读 inspect 原计划并核对完整前后像，不自动重放", plan.fingerprint)))?;
    Ok(BusinessAdoptionOutcome {
        already_applied: false,
        registered: selected.len(),
        metadata_ddl_attempted,
    })
}

async fn prepare_locked(
    database: &DatabaseTransaction,
    plan: &BusinessAdoptionPlan,
    migrations: &[&dyn BusinessMigration],
    selected: &[&dyn BusinessMigration],
    created_metadata: bool,
) -> AppResult<BusinessAdoptionState> {
    state::verify_identity(database, plan).await?;
    ledger::verify_known_records(database, plan.manifest.scope, migrations).await?;
    ledger::write::lock_business_tables(
        database,
        plan.manifest
            .tables
            .iter()
            .map(|table| table.table.as_str()),
    )
    .await?;
    state::verify_tables(database, selected).await?;
    for migration in selected {
        ledger::verify_ownership(database, *migration).await?;
    }
    let snapshot = ledger::load_snapshot(database).await?;
    state::classify(plan, selected, &snapshot, created_metadata)
}

async fn write_records(
    database: &DatabaseTransaction,
    plan: &BusinessAdoptionPlan,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<()> {
    for migration in migrations {
        ledger::insert(
            database,
            *migration,
            BusinessMigrationOrigin::Adopted,
            Some(&plan.fingerprint),
        )
        .await?;
    }
    let snapshot = ledger::load_snapshot(database).await?;
    if state::classify(plan, migrations, &snapshot, true)? != BusinessAdoptionState::Applied {
        return Err(AppError::Config(
            "接管写入后的完整账本后像未通过，拒绝提交".into(),
        ));
    }
    Ok(())
}

async fn rollback_error(
    transaction: DatabaseTransaction,
    error: AppError,
    metadata_ddl_attempted: bool,
) -> AppResult<BusinessAdoptionOutcome> {
    match transaction.rollback().await {
        Ok(()) => Err(AppError::Database(format!(
            "接管未完成：{error}；本次元数据事务已回滚，可能已创建空账本={metadata_ddl_attempted}；业务表未修改，先核对原计划"
        ))),
        Err(rollback) => Err(AppError::Database(format!(
            "接管失败：{error}；元数据回滚结果未知：{rollback}，可能已创建空账本={metadata_ddl_attempted}；保留现场，先只读核对，不自动重放"
        ))),
    }
}
