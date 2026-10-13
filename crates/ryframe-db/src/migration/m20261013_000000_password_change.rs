use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 新基线已有该列；升级已有库时不重置正常账号的密码或改密状态。
        if !manager
            .has_column("sys_user", "password_change_required")
            .await?
        {
            manager.get_connection().execute_unprepared(
                "ALTER TABLE `sys_user` ADD COLUMN `password_change_required` TINYINT(1) NOT NULL DEFAULT 0 COMMENT '首次登录必须修改密码'",
            ).await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "密码状态迁移不支持回滚，请追加 roll-forward 迁移修复".into(),
        ))
    }
}
