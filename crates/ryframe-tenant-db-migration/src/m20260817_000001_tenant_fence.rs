use sea_orm::DbBackend;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data fence migration requires MySQL".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE TABLE IF NOT EXISTS `biz_tenant_fence` (
                    `tenant_id` VARCHAR(64) NOT NULL,
                    `target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `placement_generation` BIGINT NOT NULL,
                    `state` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    PRIMARY KEY (`tenant_id`),
                    KEY `idx_biz_tenant_fence_state` (`state`, `tenant_id`),
                    CONSTRAINT `ck_biz_tenant_fence_generation` CHECK (`placement_generation` > 0),
                    CONSTRAINT `ck_biz_tenant_fence_state` CHECK (`state` IN ('active', 'frozen'))
                ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据写入围栏'"#,
            )
            .await?;

        // shared-control 具备 sys_tenant；dedicated 目标不会执行此初始化。
        if manager.has_table("sys_tenant").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"INSERT INTO `biz_tenant_fence`
                        (`tenant_id`, `target_key`, `placement_generation`, `state`, `switch_token`, `updated_at`)
                       SELECT `tenant_id`, 'shared-control', 1, 'active',
                              SHA2(CONCAT('ryframe:tenant-data:shared-control:v1:', `tenant_id`), 256),
                              CURRENT_TIMESTAMP(6)
                       FROM `sys_tenant`
                       ON DUPLICATE KEY UPDATE
                         `switch_token` = IF(
                           `target_key` = 'shared-control'
                           AND `placement_generation` = 1
                           AND `state` = 'active',
                           VALUES(`switch_token`),
                           `switch_token`
                         )"#,
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("biz_tenant_fence"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
