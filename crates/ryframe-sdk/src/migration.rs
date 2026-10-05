use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde::Serialize;
use std::sync::Arc;

use ryframe_kernel::{AppError, AppResult};

const BUSINESS_MIGRATION_LEDGER: &str = "ryframe_business_migration";

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
    /// 应用迁移前的规范化 schema 指纹；首次迁移可以为空。
    fn before_fingerprint(&self) -> Option<&'static str>;
    /// 应用迁移后的规范化 schema 指纹。
    fn after_fingerprint(&self) -> &'static str;
    fn scope(&self) -> BusinessMigrationScope;
    async fn up(&self, database: &DatabaseConnection) -> AppResult<()>;
    async fn verify(&self, database: &DatabaseConnection) -> AppResult<()>;
    async fn status(&self, database: &DatabaseConnection) -> AppResult<MigrationState>;
}

/// 把普通 SeaORM 迁移接入 RyFrame 的校验账本。
pub struct SeaOrmBusinessMigration {
    module: &'static str,
    id: &'static str,
    checksum: &'static str,
    before_fingerprint: Option<&'static str>,
    after_fingerprint: &'static str,
    scope: BusinessMigrationScope,
    migration: Arc<dyn MigrationTrait>,
}

impl SeaOrmBusinessMigration {
    pub fn new(
        module: &'static str,
        id: &'static str,
        checksum: &'static str,
        before_fingerprint: Option<&'static str>,
        after_fingerprint: &'static str,
        scope: BusinessMigrationScope,
        migration: impl MigrationTrait + 'static,
    ) -> Self {
        Self {
            module,
            id,
            checksum,
            before_fingerprint,
            after_fingerprint,
            scope,
            migration: Arc::new(migration),
        }
    }

    async fn applied_checksum(&self, database: &DatabaseConnection) -> AppResult<Option<String>> {
        if !ledger_exists(database).await? {
            return Ok(None);
        }
        let row = database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                format!(
                    "SELECT checksum FROM {BUSINESS_MIGRATION_LEDGER} WHERE module_name = ? AND migration_id = ?"
                ),
                [self.module.into(), self.id.into()],
            ))
            .await
            .map_err(database_error)?;
        row.map(|row| row.try_get("", "checksum").map_err(database_error))
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

    fn before_fingerprint(&self) -> Option<&'static str> {
        self.before_fingerprint
    }

    fn after_fingerprint(&self) -> &'static str {
        self.after_fingerprint
    }

    fn scope(&self) -> BusinessMigrationScope {
        self.scope
    }

    async fn up(&self, database: &DatabaseConnection) -> AppResult<()> {
        ensure_ledger(database).await?;
        if let Some(checksum) = self.applied_checksum(database).await? {
            return matching_checksum(self, &checksum);
        }
        self.migration
            .up(&SchemaManager::new(database))
            .await
            .map_err(database_error)?;
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
                    self.before_fingerprint.into(),
                    self.after_fingerprint.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        Ok(())
    }

    async fn verify(&self, database: &DatabaseConnection) -> AppResult<()> {
        let checksum = self.applied_checksum(database).await?.ok_or_else(|| {
            AppError::Config(format!("业务迁移 {}/{} 尚未应用", self.module, self.id))
        })?;
        matching_checksum(self, &checksum)
    }

    async fn status(&self, database: &DatabaseConnection) -> AppResult<MigrationState> {
        let applied = usize::from(self.applied_checksum(database).await?.is_some());
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
            format!(
                "CREATE TABLE IF NOT EXISTS {BUSINESS_MIGRATION_LEDGER} (module_name VARCHAR(64) NOT NULL, migration_id VARCHAR(128) NOT NULL, checksum CHAR(64) NOT NULL, before_fingerprint CHAR(64) NULL, after_fingerprint CHAR(64) NOT NULL, applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), PRIMARY KEY (module_name, migration_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci"
            ),
        ))
        .await
        .map_err(database_error)?;
    Ok(())
}

fn matching_checksum(migration: &SeaOrmBusinessMigration, actual: &str) -> AppResult<()> {
    if actual == migration.checksum {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "业务迁移 {}/{} 校验值已变化",
            migration.module, migration.id
        )))
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(format!("业务迁移数据库操作失败：{error}"))
}
