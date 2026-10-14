use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde::Serialize;
use std::sync::Arc;

use ryframe_kernel::{AppError, AppResult};

const BUSINESS_MIGRATION_LEDGER: &str = "ryframe_business_migration";
const BUSINESS_MIGRATION_LEDGER_DDL: &str = "CREATE TABLE ryframe_business_migration (module_name VARCHAR(64) NOT NULL, migration_id VARCHAR(128) NOT NULL, checksum CHAR(64) NOT NULL, before_fingerprint CHAR(64) NULL, after_fingerprint CHAR(64) NOT NULL, applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), PRIMARY KEY (module_name, migration_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci";

mod export;
mod schema;
pub use export::business_initialization_statements;
pub(crate) use schema::validate_migrations;
pub use schema::{BusinessMigrationSchema, BusinessTableSchema, verify_business_migrations};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessMigrationScope {
    Control,
    Tenant,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationState {
    pub applied: usize,
    pub expected: usize,
    pub missing: Vec<String>,
}

impl MigrationState {
    pub fn is_current(&self) -> bool {
        self.missing.is_empty() && self.applied == self.expected
    }
}

#[async_trait]
pub trait BusinessMigration: Send + Sync {
    fn module(&self) -> &'static str;
    /// 模块内稳定且按时间递增的迁移标识。
    fn id(&self) -> &'static str;
    /// 迁移源码和规范化 schema 的稳定校验值。
    fn checksum(&self) -> &'static str;
    fn schema(&self) -> &BusinessMigrationSchema;
    fn scope(&self) -> BusinessMigrationScope;
    /// 与 up 使用同一来源的有序 SQL；无可导出描述时拒绝完整 SQL 初始化。
    fn initialization_statements(&self) -> AppResult<&'static [&'static str]> {
        Err(AppError::Config(format!(
            "业务迁移 {}/{} 未提供初始化 SQL；请使用 control up，或为迁移提供与 up 同源的 SQL 描述，尚未写入导出文件",
            self.module(),
            self.id(),
        )))
    }
    /// 执行单条迁移；整组执行结束后使用 verify_business_migrations 校验当前实际结构。
    async fn up(&self, database: &DatabaseConnection) -> AppResult<()>;
    /// 只校验历史账本元数据；历史表结构可能已被后续向前迁移合法改变。
    async fn verify_ledger(&self, database: &DatabaseConnection) -> AppResult<()>;
    async fn status(&self, database: &DatabaseConnection) -> AppResult<MigrationState>;
}

/// 把普通 SeaORM 迁移接入 RyFrame 的校验账本。
pub struct SeaOrmBusinessMigration {
    module: &'static str,
    id: &'static str,
    checksum: &'static str,
    schema: BusinessMigrationSchema,
    scope: BusinessMigrationScope,
    migration: Arc<dyn MigrationTrait>,
    initialization_sql: Option<&'static [&'static str]>,
}

impl SeaOrmBusinessMigration {
    pub fn new(
        module: &'static str,
        id: &'static str,
        checksum: &'static str,
        schema: BusinessMigrationSchema,
        scope: BusinessMigrationScope,
        migration: impl MigrationTrait + 'static,
    ) -> Self {
        Self {
            module,
            id,
            checksum,
            schema,
            scope,
            migration: Arc::new(migration),
            initialization_sql: None,
        }
    }

    pub fn with_initialization_sql(mut self, statements: &'static [&'static str]) -> Self {
        self.initialization_sql = Some(statements);
        self
    }

    async fn applied_state(
        &self,
        database: &DatabaseConnection,
    ) -> AppResult<Option<AppliedMigration>> {
        if !ledger_exists(database).await? {
            return Ok(None);
        }
        let row = database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT checksum, before_fingerprint, after_fingerprint FROM {BUSINESS_MIGRATION_LEDGER} WHERE module_name = ? AND migration_id = ?"
                ),
                [self.module.into(), self.id.into()],
            ))
            .await
            .map_err(database_error)?;
        row.map(|row| {
            Ok(AppliedMigration {
                checksum: row.try_get("", "checksum").map_err(database_error)?,
                before: row
                    .try_get("", "before_fingerprint")
                    .map_err(database_error)?,
                after: row
                    .try_get("", "after_fingerprint")
                    .map_err(database_error)?,
            })
        })
        .transpose()
    }
}

#[async_trait]
impl BusinessMigration for SeaOrmBusinessMigration {
    fn module(&self) -> &'static str {
        self.module
    }

    fn id(&self) -> &'static str {
        self.id
    }

    fn checksum(&self) -> &'static str {
        self.checksum
    }

    fn schema(&self) -> &BusinessMigrationSchema {
        &self.schema
    }

    fn scope(&self) -> BusinessMigrationScope {
        self.scope
    }

    fn initialization_statements(&self) -> AppResult<&'static [&'static str]> {
        self.initialization_sql.ok_or_else(|| AppError::Config(format!(
            "业务迁移 {}/{} 未提供初始化 SQL；使用 control up，或通过 with_initialization_sql 提供与 up 同源的 SQL，尚未写入导出文件",
            self.module, self.id,
        )))
    }

    async fn up(&self, database: &DatabaseConnection) -> AppResult<()> {
        schema::validate_migration(self)?;
        if let Some(state) = self.applied_state(database).await? {
            return matching_state(self, &state);
        }
        self.schema.verify_before(database).await?;
        ensure_ledger(database).await?;
        self.migration
            .up(&SchemaManager::new(database))
            .await
            .map_err(|error| AppError::Database(format!("业务迁移 {}/{} 执行失败：{error}；DDL 可能部分提交，未登记本次账本；先核对前后结构，禁止自动重放", self.module, self.id)))?;
        self.schema.verify_after(database).await.map_err(|error| {
            AppError::Config(format!(
                "{error}；迁移 DDL 已执行但结构校验未通过，未登记本次账本"
            ))
        })?;
        database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "INSERT INTO {BUSINESS_MIGRATION_LEDGER} (module_name, migration_id, checksum, before_fingerprint, after_fingerprint) VALUES (?, ?, ?, ?, ?)"
                ),
                [
                    self.module.into(),
                    self.id.into(),
                    self.checksum.into(),
                    self.schema.before_fingerprint.into(),
                    self.schema.after_fingerprint.into(),
                ],
            ))
            .await
            .map_err(|error| AppError::Database(format!("业务迁移 {}/{} 的 DDL 已执行并通过结构校验，但账本写入失败：{error}；账本提交结果需重新核对，不自动重放", self.module, self.id)))?;
        Ok(())
    }

    async fn verify_ledger(&self, database: &DatabaseConnection) -> AppResult<()> {
        schema::validate_migration(self)?;
        let state = self.applied_state(database).await?.ok_or_else(|| {
            AppError::Config(format!("业务迁移 {}/{} 尚未应用", self.module, self.id))
        })?;
        matching_state(self, &state)
    }

    async fn status(&self, database: &DatabaseConnection) -> AppResult<MigrationState> {
        schema::validate_migration(self)?;
        let state = self.applied_state(database).await?;
        if let Some(state) = &state {
            matching_state(self, state)?;
        }
        let applied = usize::from(state.is_some());
        Ok(MigrationState {
            applied,
            expected: 1,
            missing: (applied == 0)
                .then(|| self.id.to_owned())
                .into_iter()
                .collect(),
        })
    }
}

async fn ledger_exists(database: &DatabaseConnection) -> AppResult<bool> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            format!(
                "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = '{BUSINESS_MIGRATION_LEDGER}') AS ledger_exists"
            ),
        ))
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::Database("无法读取业务迁移账本状态".into()))?;
    row.try_get::<i64>("", "ledger_exists")
        .map(|exists| exists != 0)
        .map_err(database_error)
}

async fn ensure_ledger(database: &DatabaseConnection) -> AppResult<()> {
    database
        .execute_raw(Statement::from_string(
            DbBackend::MySql,
            BUSINESS_MIGRATION_LEDGER_DDL.replacen("CREATE TABLE", "CREATE TABLE IF NOT EXISTS", 1),
        ))
        .await
        .map_err(database_error)?;
    Ok(())
}

struct AppliedMigration {
    checksum: String,
    before: Option<String>,
    after: String,
}

fn matching_state(migration: &SeaOrmBusinessMigration, actual: &AppliedMigration) -> AppResult<()> {
    let mut changed = Vec::new();
    if actual.checksum != migration.checksum {
        changed.push("checksum");
    }
    if actual.before.as_deref() != migration.schema.before_fingerprint {
        changed.push("before_fingerprint");
    }
    if actual.after != migration.schema.after_fingerprint {
        changed.push("after_fingerprint");
    }
    if changed.is_empty() {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "业务迁移 {}/{} 的账本不一致，变化字段：{}；核对迁移来源与账本，禁止自动覆盖或重放",
            migration.module,
            migration.id,
            changed.join("、")
        )))
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(format!("业务迁移数据库操作失败：{error}"))
}
