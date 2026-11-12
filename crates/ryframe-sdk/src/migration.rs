use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use ryframe_kernel::{AppError, AppResult};

mod adoption;
mod export;
mod ledger;
mod schema;
pub use adoption::{
    BusinessAdoptionLedger, BusinessAdoptionManifest, BusinessAdoptionMigration,
    BusinessAdoptionOutcome, BusinessAdoptionPlan, BusinessAdoptionState, BusinessAdoptionTable,
    BusinessAdoptionTarget, apply_business_adoption, inspect_business_adoption,
    preview_business_adoption,
};
pub use export::{BusinessInitializationSql, business_initialization_statements};
pub(crate) use schema::validate_migrations;
pub use schema::{
    BusinessMigrationSchema, BusinessTableSchema, validate_business_migration_ledger,
    verify_business_migrations, verify_business_migrations_in_session,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessMigrationScope {
    Control,
    Tenant,
}

impl BusinessMigrationScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Tenant => "tenant",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BusinessMigrationOrigin {
    Executed,
    Adopted,
}

impl BusinessMigrationOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Executed => "executed",
            Self::Adopted => "adopted",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationState {
    pub applied: usize,
    pub expected: usize,
    pub missing: Vec<String>,
    pub origin: Option<BusinessMigrationOrigin>,
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
    /// 在组合根持有迁移互斥的原会话执行，不额外租用池连接。
    async fn up_in_session(&self, database: &mut DatabaseTransaction) -> AppResult<()>;
    /// 只校验历史账本元数据；历史表结构可能已被后续向前迁移合法改变。
    async fn verify_ledger(&self, database: &DatabaseConnection) -> AppResult<()>;
    async fn verify_ledger_in_session(&self, database: &DatabaseTransaction) -> AppResult<()>;
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
        if self.execute_ddl(database).await? {
            ledger::record_executed(database, self)
                .await
                .map_err(|error| self.ledger_failure(error))?;
        }
        Ok(())
    }

    async fn up_in_session(&self, database: &mut DatabaseTransaction) -> AppResult<()> {
        if database.is_open() {
            return Err(AppError::Config(
                "同会话业务迁移必须从已结束的根事务开始，不能提交调用方的活跃事务".into(),
            ));
        }
        if self.execute_ddl(database).await? {
            ledger::write::record_in_session(database, self)
                .await
                .map_err(|error| self.ledger_failure(error))?;
        }
        Ok(())
    }

    async fn verify_ledger(&self, database: &DatabaseConnection) -> AppResult<()> {
        self.verify_ledger_on(database).await
    }

    async fn verify_ledger_in_session(&self, database: &DatabaseTransaction) -> AppResult<()> {
        self.verify_ledger_on(database).await
    }
    async fn status(&self, database: &DatabaseConnection) -> AppResult<MigrationState> {
        schema::validate_migration(self)?;
        let state = ledger::state(database, self).await?;
        if let Some(state) = &state {
            state.verify(self)?;
        }
        ledger::verify_ownership(database, self).await?;
        let applied = usize::from(state.is_some());
        Ok(MigrationState {
            applied,
            expected: 1,
            origin: state.as_ref().map(|record| record.origin),
            missing: (applied == 0)
                .then(|| self.id.to_owned())
                .into_iter()
                .collect(),
        })
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(format!("业务迁移数据库操作失败：{error}"))
}

impl SeaOrmBusinessMigration {
    fn ledger_failure(&self, error: AppError) -> AppError {
        AppError::Database(format!(
            "业务迁移 {}/{} 的 DDL 已执行并通过结构校验，但账本写入失败或提交结果未知。先核对权威账本与实际结构，不自动重放。底层错误 {error}",
            self.module, self.id
        ))
    }

    async fn execute_ddl<C>(&self, database: &C) -> AppResult<bool>
    where
        C: ConnectionTrait,
        for<'c> &'c C: sea_orm_migration::IntoSchemaManagerConnection<'c>,
    {
        schema::validate_migration(self)?;
        if let Some(state) = ledger::state(database, self).await? {
            state.verify(self)?;
            ledger::verify_ownership(database, self).await?;
            return Ok(false);
        }
        ledger::verify_ownership(database, self).await?;
        self.schema.verify_before(database).await?;
        ledger::ensure(database).await?;
        self.migration.up(&SchemaManager::new(database)).await.map_err(|error|
            AppError::Database(format!("业务迁移 {}/{} 执行失败：{error}；DDL可能部分提交，未登记本次账本；先核对前后结构，禁止自动重放",self.module,self.id)))?;
        self.schema.verify_after(database).await.map_err(|error| {
            AppError::Config(format!(
                "{error}；迁移DDL已执行但结构校验未通过，未登记本次账本"
            ))
        })?;
        Ok(true)
    }

    async fn verify_ledger_on<C: ConnectionTrait>(&self, database: &C) -> AppResult<()> {
        schema::validate_migration(self)?;
        ledger::state(database, self)
            .await?
            .ok_or_else(|| {
                AppError::Config(format!("业务迁移 {}/{} 尚未应用", self.module, self.id))
            })?
            .verify(self)?;
        ledger::verify_ownership(database, self).await
    }
}
