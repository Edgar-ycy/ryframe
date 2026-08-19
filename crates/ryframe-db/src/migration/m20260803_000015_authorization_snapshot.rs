use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

const USER_TABLE: &str = "sys_user";
const TENANT_TABLE: &str = "sys_tenant";
const LEGACY_USER_VERSION_COLUMN: &str = "auth_version";
const USER_VERSION_COLUMN: &str = "authorization_version";
const TENANT_EPOCH_COLUMN: &str = "authorization_epoch";

/// 将用户授权版本收敛为新列，并为租户级授权规则建立 epoch。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        migrate_user_authorization_version(manager).await?;
        add_tenant_authorization_epoch(manager).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "授权版本迁移不可逆：回滚会重新引入已删除的旧列契约".into(),
        ))
    }
}

async fn migrate_user_authorization_version(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table(USER_TABLE).await? {
        return Ok(());
    }

    let has_legacy = manager
        .has_column(USER_TABLE, LEGACY_USER_VERSION_COLUMN)
        .await?;
    let has_current = manager.has_column(USER_TABLE, USER_VERSION_COLUMN).await?;

    match (has_legacy, has_current) {
        (true, false) => {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE `sys_user` \
                     CHANGE COLUMN `auth_version` `authorization_version` \
                     INT NOT NULL DEFAULT 1 COMMENT '用户授权版本，权限或凭据变更时递增'",
                )
                .await?;
        }
        (false, false) => {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new(USER_TABLE))
                        .add_column(
                            ColumnDef::new(Alias::new(USER_VERSION_COLUMN))
                                .integer()
                                .not_null()
                                .default(1),
                        )
                        .to_owned(),
                )
                .await?;
        }
        (true, true) => {
            return Err(DbErr::Custom(
                "sys_user 同时存在 auth_version 与 authorization_version，拒绝猜测数据来源".into(),
            ));
        }
        (false, true) => {}
    }

    Ok(())
}

async fn add_tenant_authorization_epoch(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if manager.has_table(TENANT_TABLE).await?
        && !manager
            .has_column(TENANT_TABLE, TENANT_EPOCH_COLUMN)
            .await?
    {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new(TENANT_TABLE))
                    .add_column(
                        ColumnDef::new(Alias::new(TENANT_EPOCH_COLUMN))
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
