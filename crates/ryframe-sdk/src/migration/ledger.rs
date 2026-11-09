use std::collections::BTreeMap;

use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement};

use super::{BusinessMigration, BusinessMigrationOrigin, BusinessMigrationScope, database_error};

pub(super) mod write;
pub(super) use write::record_executed;
mod snapshot;
pub(super) use snapshot::{LedgerSnapshot, load_snapshot};

pub(super) const NAME: &str = "ryframe_business_migration";
pub(super) const DDL: &str = "CREATE TABLE ryframe_business_migration (scope VARCHAR(8) NOT NULL, module_name VARCHAR(64) NOT NULL, migration_id VARCHAR(128) NOT NULL, checksum CHAR(64) NOT NULL, before_fingerprint CHAR(64) NULL, after_fingerprint CHAR(64) NOT NULL, origin VARCHAR(16) NOT NULL, tables_json JSON NOT NULL, adoption_hash CHAR(64) NULL, applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), PRIMARY KEY (scope, module_name, migration_id), CONSTRAINT chk_business_origin CHECK ((origin='executed' AND adoption_hash IS NULL) OR (origin='adopted' AND adoption_hash IS NOT NULL))) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci";

#[derive(serde::Serialize)]
pub(super) struct AppliedMigration {
    pub scope: String,
    pub module: String,
    pub id: String,
    pub checksum: String,
    pub before: Option<String>,
    pub after: String,
    pub origin: BusinessMigrationOrigin,
    pub tables: Vec<String>,
    pub adoption_hash: Option<String>,
}

impl AppliedMigration {
    fn from_row(row: QueryResult) -> AppResult<Self> {
        let origin: String = row.try_get("", "origin").map_err(database_error)?;
        let origin = match origin.as_str() {
            "executed" => BusinessMigrationOrigin::Executed,
            "adopted" => BusinessMigrationOrigin::Adopted,
            _ => {
                return Err(AppError::Config(
                    "业务迁移账本 origin 非法；拒绝把未知来源当作已执行".into(),
                ));
            }
        };
        let tables: serde_json::Value = row.try_get("", "tables_json").map_err(database_error)?;
        Ok(Self {
            scope: row.try_get("", "scope").map_err(database_error)?,
            module: row.try_get("", "module_name").map_err(database_error)?,
            id: row.try_get("", "migration_id").map_err(database_error)?,
            checksum: row.try_get("", "checksum").map_err(database_error)?,
            before: row
                .try_get("", "before_fingerprint")
                .map_err(database_error)?,
            after: row
                .try_get("", "after_fingerprint")
                .map_err(database_error)?,
            origin,
            tables: serde_json::from_value(tables)
                .map_err(|_| AppError::Config("业务迁移账本 tables_json 必须是表名数组".into()))?,
            adoption_hash: row.try_get("", "adoption_hash").map_err(database_error)?,
        })
    }

    pub(super) fn verify(&self, migration: &dyn BusinessMigration) -> AppResult<()> {
        let mut changed = Vec::new();
        if self.scope != migration.scope().as_str() {
            changed.push("scope");
        }
        if self.module != migration.module() {
            changed.push("module_name");
        }
        if self.id != migration.id() {
            changed.push("migration_id");
        }
        if self.checksum != migration.checksum() {
            changed.push("checksum");
        }
        if self.before.as_deref() != migration.schema().before_fingerprint {
            changed.push("before_fingerprint");
        }
        if self.after != migration.schema().after_fingerprint {
            changed.push("after_fingerprint");
        }
        if self.tables != table_names(migration) {
            changed.push("tables_json");
        }
        let valid_origin = match self.origin {
            BusinessMigrationOrigin::Executed => self.adoption_hash.is_none(),
            BusinessMigrationOrigin::Adopted => self
                .adoption_hash
                .as_deref()
                .is_some_and(super::schema::valid_fingerprint),
        };
        if !valid_origin {
            changed.push("origin/adoption_hash");
        }
        if changed.is_empty() {
            return Ok(());
        }
        Err(AppError::Config(format!(
            "业务迁移 {}/{} 的账本不一致，变化字段：{}；核对迁移来源与账本，禁止自动覆盖或重放",
            migration.module(),
            migration.id(),
            changed.join("、")
        )))
    }
}

pub(super) fn table_names(migration: &dyn BusinessMigration) -> Vec<String> {
    let mut names = migration
        .schema()
        .tables
        .iter()
        .map(|table| table.table.to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

pub(super) async fn exists<C: ConnectionTrait>(database: &C) -> AppResult<bool> {
    let row = database.query_one_raw(Statement::from_string(DbBackend::MySql,
        format!("SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema=DATABASE() AND table_name='{NAME}') AS ledger_exists")))
        .await.map_err(database_error)?.ok_or_else(|| AppError::Database("无法读取业务迁移账本状态".into()))?;
    row.try_get::<i64>("", "ledger_exists")
        .map(|value| value != 0)
        .map_err(database_error)
}

pub(super) async fn ensure<C: ConnectionTrait>(database: &C) -> AppResult<()> {
    database
        .execute_raw(Statement::from_string(
            DbBackend::MySql,
            DDL.replacen("CREATE TABLE", "CREATE TABLE IF NOT EXISTS", 1),
        ))
        .await
        .map_err(database_error)?;
    Ok(())
}

pub(super) async fn state<C: ConnectionTrait>(
    database: &C,
    migration: &dyn BusinessMigration,
) -> AppResult<Option<AppliedMigration>> {
    if !exists(database).await? {
        return Ok(None);
    }
    database.query_one_raw(Statement::from_sql_and_values(DbBackend::MySql,
        format!("SELECT scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash FROM {NAME} WHERE scope=? AND module_name=? AND migration_id=?"),
        [migration.scope().as_str().into(), migration.module().into(), migration.id().into()]))
        .await.map_err(ledger_read_error)?.map(AppliedMigration::from_row).transpose()
}

pub(super) async fn insert<C: ConnectionTrait>(
    database: &C,
    migration: &dyn BusinessMigration,
    origin: BusinessMigrationOrigin,
    adoption_hash: Option<&str>,
) -> AppResult<()> {
    database.execute_raw(Statement::from_sql_and_values(DbBackend::MySql,
        format!("INSERT INTO {NAME} (scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash) VALUES (?,?,?,?,?,?,?,?,?)"),
        [migration.scope().as_str().into(), migration.module().into(), migration.id().into(), migration.checksum().into(), migration.schema().before_fingerprint.into(), migration.schema().after_fingerprint.into(), origin.as_str().into(), serde_json::json!(table_names(migration)).into(), adoption_hash.into()]))
        .await.map_err(database_error)?;
    Ok(())
}

pub(super) async fn verify_ownership<C: ConnectionTrait>(
    database: &C,
    migration: &dyn BusinessMigration,
) -> AppResult<()> {
    if !exists(database).await? {
        return Ok(());
    }
    for table in migration.schema().tables {
        let rows = database.query_all_raw(Statement::from_sql_and_values(DbBackend::MySql,
            format!("SELECT scope,module_name FROM {NAME} WHERE JSON_CONTAINS(tables_json, JSON_QUOTE(?))"), [table.table.into()]))
            .await.map_err(ledger_read_error)?;
        for row in rows {
            let scope: String = row.try_get("", "scope").map_err(database_error)?;
            let module: String = row.try_get("", "module_name").map_err(database_error)?;
            if scope != migration.scope().as_str() || module != migration.module() {
                return Err(AppError::Config(format!(
                    "业务表 {} 已登记归属 {scope}/{module}，不能由 {}/{} 重复迁移或接管；本次尚未写入",
                    table.table,
                    migration.scope().as_str(),
                    migration.module()
                )));
            }
        }
    }
    Ok(())
}

pub(super) async fn verify_known_records<C: ConnectionTrait>(
    database: &C,
    scope: BusinessMigrationScope,
    migrations: &[&dyn BusinessMigration],
) -> AppResult<()> {
    if !exists(database).await? {
        return Ok(());
    }
    let expected = migrations
        .iter()
        .map(|migration| ((migration.module(), migration.id()), *migration))
        .collect::<BTreeMap<_, _>>();
    let rows = database.query_all_raw(Statement::from_sql_and_values(DbBackend::MySql,
        format!("SELECT scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash FROM {NAME} WHERE scope=? OR scope NOT IN ('control','tenant')"), [scope.as_str().into()]))
        .await.map_err(ledger_read_error)?;
    for row in rows {
        let record = AppliedMigration::from_row(row)?;
        let migration = expected
            .get(&(record.module.as_str(), record.id.as_str()))
            .ok_or_else(|| {
                AppError::Config(format!(
                    "{} 范围存在未注册的业务迁移 {}/{}；检查模块注册及账本来源，禁止自动删除或重放",
                    scope.as_str(),
                    record.module,
                    record.id
                ))
            })?;
        record.verify(*migration)?;
    }
    Ok(())
}

fn ledger_read_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(format!(
        "业务迁移账本读取失败：{error}；核对 scope、origin、tables_json 等当前字段；旧开发账本需先备份并审阅，不自动转换或清空"
    ))
}
