use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 新基线已包含该列；已有开发库只追加独立计数，不扫描或改写身份数据。
        if !manager
            .has_column("sys_tenant", "data_scope_version")
            .await?
        {
            manager.get_connection().execute_unprepared(
                "ALTER TABLE `sys_tenant` ADD COLUMN `data_scope_version` BIGINT NOT NULL DEFAULT 1 \
                 COMMENT '租户数据范围投影权威版本' AFTER `authorization_epoch`",
            ).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "数据范围投影版本迁移不支持回滚，请追加 roll-forward 迁移修复".into(),
        ))
    }
}
