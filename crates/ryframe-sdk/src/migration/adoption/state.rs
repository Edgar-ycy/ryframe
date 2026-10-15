use std::collections::BTreeMap;

use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};

use super::super::{
    BusinessMigration, BusinessMigrationOrigin, BusinessMigrationScope, database_error, ledger,
    schema,
};
use super::model::{fingerprint, schema_fingerprint};
use super::{
    BusinessAdoptionMigration, BusinessAdoptionPlan, BusinessAdoptionState, BusinessAdoptionTable,
    BusinessAdoptionTarget,
};

pub(super) fn selected<'a>(
    scope: BusinessMigrationScope,
    module: &str,
    migrations: &[&'a dyn BusinessMigration],
) -> AppResult<Vec<&'a dyn BusinessMigration>> {
    schema::validate_migrations(migrations)?;
    if migrations
        .iter()
        .any(|migration| migration.scope() != scope)
    {
        return Err(AppError::Config(
            "接管只接受一个明确范围的完整注册迁移；分别处理 control/tenant".into(),
        ));
    }
    let selected = migrations
        .iter()
        .copied()
        .filter(|migration| migration.module() == module)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(AppError::Config(format!(
            "模块 {module} 在 {} 范围没有已注册业务迁移，尚未写入",
            scope.as_str()
        )));
    }
    Ok(selected)
}

pub(super) fn bindings(migrations: &[&dyn BusinessMigration]) -> Vec<BusinessAdoptionMigration> {
    migrations
        .iter()
        .map(|migration| BusinessAdoptionMigration {
            id: migration.id().to_owned(),
            checksum: migration.checksum().to_owned(),
            before_fingerprint: migration.schema().before_fingerprint.map(str::to_owned),
            after_fingerprint: migration.schema().after_fingerprint.to_owned(),
            tables: ledger::table_names(*migration),
        })
        .collect()
}

pub(super) fn latest(
    migrations: &[&dyn BusinessMigration],
) -> BTreeMap<&'static str, &'static str> {
    migrations
        .iter()
        .flat_map(|migration| {
            migration
                .schema()
                .tables
                .iter()
                .map(|table| (table.table, table.after))
        })
        .collect()
}

pub(super) fn table_bindings(migrations: &[&dyn BusinessMigration]) -> Vec<BusinessAdoptionTable> {
    latest(migrations)
        .into_iter()
        .map(|(table, canonical)| BusinessAdoptionTable {
            table: table.to_owned(),
            fingerprint: schema_fingerprint(canonical),
        })
        .collect()
}

pub(super) fn validate_source<'a>(
    plan: &BusinessAdoptionPlan,
    migrations: &[&'a dyn BusinessMigration],
) -> AppResult<Vec<&'a dyn BusinessMigration>> {
    plan.validate()?;
    let selected = selected(plan.manifest.scope, &plan.manifest.module, migrations)?;
    if bindings(&selected) != plan.manifest.migrations
        || table_bindings(&selected) != plan.manifest.tables
    {
        return Err(AppError::Config(
            "接管计划与当前编译的迁移、模型结构或注册顺序不一致；重新预览并审阅，尚未写入".into(),
        ));
    }
    Ok(selected)
}

#[derive(FromQueryResult)]
struct TargetRow {
    server_uuid: String,
    database: String,
}

pub(super) async fn identity<C: ConnectionTrait>(
    database: &C,
) -> AppResult<BusinessAdoptionTarget> {
    let row = TargetRow::find_by_statement(Statement::from_string(DbBackend::MySql,
        "SELECT LOWER(@@server_uuid) AS server_uuid, CASE WHEN @@lower_case_table_names=0 THEN DATABASE() ELSE LOWER(DATABASE()) END AS `database`"))
        .one(database).await.map_err(database_error)?.ok_or_else(|| AppError::Database("无法读取明确数据库的物理身份".into()))?;
    if row.server_uuid.is_empty() || row.database.is_empty() {
        return Err(AppError::Config(
            "接管目标必须包含 MySQL 实例身份与明确数据库".into(),
        ));
    }
    Ok(BusinessAdoptionTarget {
        server_uuid: row.server_uuid,
        database: row.database,
    })
}

pub(super) async fn verify_identity<C: ConnectionTrait>(
    database: &C,
    plan: &BusinessAdoptionPlan,
) -> AppResult<()> {
    let actual = identity(database).await?;
    if actual != plan.manifest.target {
        return Err(AppError::Config(format!(
            "接管物理实例或数据库与计划不一致，计划库={}，当前库={}；拒绝写入，不用配置别名猜测目标",
            plan.manifest.target.database, actual.database
        )));
    }
    Ok(())
}

pub(super) async fn verify_tables<C: ConnectionTrait>(
    database: &C,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<()> {
    for (table, expected) in latest(migrations) {
        let actual = ryframe_db::schema::canonical_table_schema(database, table)
            .await
            .map_err(database_error)?;
        if actual != expected {
            let difference = ryframe_db::schema::describe_schema_difference(expected, &actual);
            return Err(AppError::Config(format!(
                "接管表 {table} 结构不匹配：{difference}；先核对模型与已有表，不执行 DDL 或覆盖数据"
            )));
        }
    }
    Ok(())
}

pub(super) fn classify(
    plan: &BusinessAdoptionPlan,
    migrations: &[&dyn BusinessMigration],
    snapshot: &ledger::LedgerSnapshot,
    created_metadata: bool,
) -> AppResult<BusinessAdoptionState> {
    let expected = migrations
        .iter()
        .map(|migration| {
            (
                (
                    migration.scope().as_str(),
                    migration.module(),
                    migration.id(),
                ),
                *migration,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut original = Vec::new();
    let mut adopted = 0;
    for row in &snapshot.rows {
        let record = &row.record;
        if let Some(migration) = expected.get(&(
            record.scope.as_str(),
            record.module.as_str(),
            record.id.as_str(),
        )) {
            record.verify(*migration)?;
            if record.origin != BusinessMigrationOrigin::Adopted
                || record.adoption_hash.as_deref() != Some(&plan.fingerprint)
            {
                return Err(AppError::Config(
                    "接管模块已由不同操作或计划登记；核对来源，拒绝覆盖账本".into(),
                ));
            }
            adopted += 1;
        } else {
            original.push(row);
        }
    }
    if fingerprint(&original)? != plan.manifest.ledger.fingerprint {
        return Err(AppError::Config(
            "接管账本前后像不符合计划，存在其他写入、删除或修改；先核对完整现场，不自动重放".into(),
        ));
    }
    if adopted == migrations.len() && snapshot.exists {
        return Ok(BusinessAdoptionState::Applied);
    }
    if adopted != 0 {
        return Err(AppError::Config(
            "接管账本只有部分计划记录，结果不完整；保留现场，不自动补写或重放".into(),
        ));
    }
    if snapshot.exists == plan.manifest.ledger.existed
        || (created_metadata && !plan.manifest.ledger.existed && snapshot.exists)
    {
        return Ok(BusinessAdoptionState::Unapplied);
    }
    Err(AppError::Config(
        "接管账本存在性与计划不同；可能只完成过元数据建表，核对后重新生成计划，不自动重放".into(),
    ))
}
