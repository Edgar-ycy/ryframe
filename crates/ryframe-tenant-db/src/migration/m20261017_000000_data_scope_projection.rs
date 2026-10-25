use sea_orm::DbBackend;
use sea_orm_migration::prelude::*;

use super::m20260820_000000_tenant_baseline::{
    DATA_SCOPE_DEPT_DDL, DATA_SCOPE_OWNER_DDL, DATA_SCOPE_PROJECTION_DDL,
};

pub(super) fn table_statements() -> [&'static str; 3] {
    [
        DATA_SCOPE_PROJECTION_DDL,
        DATA_SCOPE_OWNER_DDL,
        DATA_SCOPE_DEPT_DDL,
    ]
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data data scope projection requires MySQL".into(),
            ));
        }
        for statement in table_statements() {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "数据范围投影迁移不支持回滚，请追加 roll-forward 迁移修复".into(),
        ))
    }
}
