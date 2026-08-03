use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("sys_user").await?
            && !manager.has_column("sys_user", "auth_version").await?
            && !manager
                .has_column("sys_user", "authorization_version")
                .await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_user"))
                        .add_column(
                            ColumnDef::new(Alias::new("auth_version"))
                                .integer()
                                .not_null()
                                .default(1),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "用户授权版本迁移不可逆：回退会破坏会话与权限失效语义，请恢复同版本备份".into(),
        ))
    }
}
