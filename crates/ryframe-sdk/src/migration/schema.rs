use std::collections::BTreeMap;

use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

use super::BusinessMigration;

/// 单张表在一次迁移前后的物理结构；初始迁移的 before 必须为空。
#[derive(Clone, Copy, Debug)]
pub struct BusinessTableSchema {
    pub table: &'static str,
    pub before: Option<&'static str>,
    pub after: &'static str,
}

/// 模型结构指纹与实际 MySQL 表描述分别保存，避免混淆源码摘要和数据库结构。
#[derive(Clone, Copy, Debug)]
pub struct BusinessMigrationSchema {
    pub before_fingerprint: Option<&'static str>,
    pub after_fingerprint: &'static str,
    pub tables: &'static [BusinessTableSchema],
}

impl BusinessMigrationSchema {
    pub(super) fn validate(&self) -> AppResult<()> {
        if self.tables.is_empty() {
            return Err(AppError::Config(
                "业务迁移缺少实际表结构描述；声明 tables 后重试，尚未修改数据库".into(),
            ));
        }
        if !valid_fingerprint(self.after_fingerprint)
            || self
                .before_fingerprint
                .is_some_and(|hash| !valid_fingerprint(hash))
        {
            return Err(AppError::Config(
                "业务迁移模型结构指纹必须是 64 位十六进制摘要".into(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for table in self.tables {
            if table.table.is_empty()
                || table.table.len() > 64
                || !table
                    .table
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !names.insert(table.table)
                || !ryframe_db::schema::validate_canonical_schema(table.table, table.after)
                || table.before.is_some_and(|schema| {
                    !ryframe_db::schema::validate_canonical_schema(table.table, schema)
                })
            {
                return Err(AppError::Config(format!(
                    "业务表 {} 的结构描述、名称或归属重复；修正迁移契约后重试，尚未修改数据库",
                    table.table
                )));
            }
        }
        Ok(())
    }

    pub(super) async fn verify_before(&self, database: &DatabaseConnection) -> AppResult<()> {
        for table in self.tables {
            if let Some(before) = table.before {
                verify_table(database, table.table, before).await?;
            } else {
                let row = database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
                    "SELECT CAST(COUNT(*) AS SIGNED) AS object_count FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name=?", [table.table.into()]))
                    .await.map_err(super::database_error)?.ok_or_else(|| AppError::Database("无法读取迁移目标表状态".into()))?;
                let count: i64 = row
                    .try_get("", "object_count")
                    .map_err(super::database_error)?;
                if count != 0 {
                    return Err(AppError::Config(format!(
                        "初始迁移目标表 {} 已存在；up 不自动接管，新建业务请选择空库，已有表需先核对结构与归属，禁止自动覆盖；尚未执行本次迁移",
                        table.table
                    )));
                }
            }
        }
        Ok(())
    }

    pub(super) async fn verify_after(&self, database: &DatabaseConnection) -> AppResult<()> {
        for table in self.tables {
            verify_table(database, table.table, table.after).await?;
        }
        Ok(())
    }
}

pub(crate) fn validate_migrations(migrations: &[&dyn BusinessMigration]) -> AppResult<()> {
    let mut seen = std::collections::BTreeSet::new();
    let mut latest = BTreeMap::new();
    for migration in migrations {
        validate_migration(*migration)?;
        if !seen.insert((migration.module(), migration.id())) {
            return Err(AppError::Config(format!(
                "业务迁移 {}/{} 重复登记",
                migration.module(),
                migration.id()
            )));
        }
        for table in migration.schema().tables {
            let key = (
                matches!(migration.scope(), super::BusinessMigrationScope::Tenant),
                table.table,
            );
            if let Some((module, previous)) = latest.insert(key, (migration.module(), table.after))
                && (module != migration.module() || table.before != Some(previous))
            {
                return Err(AppError::Config(format!(
                    "业务表 {} 的迁移归属或前后结构链不一致；修正注册顺序与 before/after 后重试",
                    table.table
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_migration(migration: &dyn BusinessMigration) -> AppResult<()> {
    migration.schema().validate()?;
    if migration.module().is_empty()
        || migration.module().chars().count() > 64
        || migration.id().is_empty()
        || migration.id().chars().count() > 128
        || !valid_fingerprint(migration.checksum())
    {
        return Err(AppError::Config("业务迁移的 module、id 或 checksum 无效；检查账本字段长度和 64 位十六进制摘要，尚未修改数据库".into()));
    }
    Ok(())
}

fn valid_fingerprint(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 核验所选迁移的账本及每张表最后一次迁移的结构，不拿历史初始结构误判后续迁移。
pub async fn verify_business_migrations(
    database: &DatabaseConnection,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<()> {
    validate_migrations(migrations)?;
    if migrations.first().is_some_and(|first| {
        migrations
            .iter()
            .any(|migration| migration.scope() != first.scope())
    }) {
        return Err(AppError::Config(
            "同一次数据库结构校验只能选择一种迁移范围；分别校验 control 与 tenant 目标".into(),
        ));
    }
    let mut latest = BTreeMap::new();
    for migration in migrations {
        migration.verify_ledger(database).await?;
        for table in migration.schema().tables {
            latest.insert(table.table, table.after);
        }
    }
    for (table, expected) in latest {
        verify_table(database, table, expected).await?;
    }
    Ok(())
}

async fn verify_table(database: &DatabaseConnection, table: &str, expected: &str) -> AppResult<()> {
    let actual = ryframe_db::schema::canonical_table_schema(database, table)
        .await
        .map_err(super::database_error)?;
    if actual == expected {
        return Ok(());
    }
    let detail = ryframe_db::schema::describe_schema_difference(expected, &actual);
    Err(AppError::Config(format!(
        "业务表 {table} 实际结构不一致：{detail}；执行 SHOW CREATE TABLE `{table}` 对照本模块迁移，审阅后追加向前修复，禁止覆盖或自动重放"
    )))
}
