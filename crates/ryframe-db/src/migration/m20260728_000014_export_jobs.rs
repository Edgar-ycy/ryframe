use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::prelude::*;

/// 从唯一控制库基线创建面向用户的异步导出任务表。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom(
                "export jobs require MySQL 8.0.16 or newer".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared(crate::migration::schema::export_job::EXPORT_JOB_DDL)
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom("export job migration is forward-only".into()))
    }
}
