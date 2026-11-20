#[cfg(feature = "migration")]
use sea_orm_migration::prelude::*;

pub(super) const TABLE: &str = "sys_notice";
pub(super) const COLUMN_DEFINITION: &str = "`revision` BIGINT UNSIGNED NOT NULL DEFAULT 1";

pub(super) fn add_column_sql() -> String {
    format!("ALTER TABLE `{TABLE}` ADD COLUMN {COLUMN_DEFINITION}")
}

#[cfg(feature = "migration")]
#[derive(DeriveMigrationName)]
pub struct Migration;

#[cfg(feature = "migration")]
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&add_column_sql())
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "追加迁移 m20261120_072911_notice_revision 不支持 down；请新建追加迁移执行 roll-forward 修复".into(),
        ))
    }
}
