use std::collections::BTreeMap;

use sea_orm::{DbBackend, DbErr, Schema, sea_query::MysqlQueryBuilder};
use sea_orm_migration::{MigrationName, MigratorTrait};

use super::m20260820_000000_tenant_baseline as baseline;

/// 同一控制库物理数据库的租户结构、数据与最后完成标记。
pub struct SharedControlInitialization {
    pub statements: Vec<String>,
    pub completion: Vec<String>,
}

pub fn shared_control_initialization() -> Result<SharedControlInitialization, DbErr> {
    let completion = completion_statements()?;
    let mut statements = table_statements()?;
    statements.extend([
        baseline::TARGET_SLOT_SEED.into(),
        baseline::SHARED_CONTROL_FENCE_SEED.into(),
    ]);
    Ok(SharedControlInitialization {
        statements,
        completion,
    })
}

fn table_statements() -> Result<Vec<String>, DbErr> {
    let ownership = ryframe_db::migration::control_ddl_statements()
        .find(|sql| {
            sql.trim_start()
                .starts_with("CREATE TABLE IF NOT EXISTS `ryframe_resource_ownership`")
        })
        .ok_or_else(|| DbErr::Custom("控制库缺少共享 ownership 定义".into()))?;
    if ownership.trim() != baseline::RESOURCE_OWNERSHIP_DDL.trim() {
        return Err(DbErr::Custom(
            "控制库与租户库 ownership 定义不同，拒绝去重导出".into(),
        ));
    }
    let mut tables = BTreeMap::new();
    let mut statements = Vec::new();
    for ddl in baseline::table_statements()
        .into_iter()
        .chain(super::m20261017_000000_data_scope_projection::table_statements())
    {
        let ddl = ddl.trim();
        let tail = ddl
            .strip_prefix("CREATE TABLE IF NOT EXISTS `")
            .ok_or_else(|| DbErr::Custom("租户初始化 DDL 不是受支持的建表定义".into()))?;
        let name = tail
            .split('`')
            .next()
            .ok_or_else(|| DbErr::Custom("租户初始化缺少表名".into()))?;
        if let Some(previous) = tables.insert(name, ddl) {
            if previous != ddl {
                return Err(DbErr::Custom(format!(
                    "租户初始化表 {name} 重复定义不同，拒绝导出"
                )));
            }
        } else if name != "ryframe_resource_ownership" {
            statements.push(format!("CREATE TABLE `{tail}"));
        }
    }
    Ok(statements)
}

fn completion_statements() -> Result<Vec<String>, DbErr> {
    let baseline_migration = baseline::Migration;
    let projection_migration = super::m20261017_000000_data_scope_projection::Migration;
    let supported = [baseline_migration.name(), projection_migration.name()];
    let migrations = super::Migrator::migrations();
    let actual = migrations
        .iter()
        .map(|migration| migration.name())
        .collect::<Vec<_>>();
    if actual != supported || actual != super::expected_migration_names().collect::<Vec<_>>() {
        return Err(DbErr::Custom(
            "租户迁移集合与离线初始化定义不一致；补齐同源导出后重试，未生成可导入 SQL".into(),
        ));
    }
    // 与 Migrator::install 使用同一 SeaORM 实体和实际账本名；不启用 if_not_exists。
    let ledger = Schema::new(DbBackend::MySql)
        .create_table_from_entity(sea_orm_migration::seaql_migrations::Entity)
        .table(super::Migrator::migration_table_name())
        .to_string(MysqlQueryBuilder);
    let mut completion = vec![ledger];
    for name in actual {
        completion.push(format!("INSERT INTO `{}` (`version`, `applied_at`) SELECT '{name}', UNIX_TIMESTAMP() FROM `ryframe_initialization_guard` WHERE `valid` = 1", super::TENANT_DATA_MIGRATION_LEDGER));
    }
    Ok(completion)
}
