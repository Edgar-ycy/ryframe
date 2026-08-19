use sea_orm::DbBackend;
use sea_orm_migration::prelude::*;

/// dedicated 目标的数据库内固定互斥槽。
///
/// 占用安全只依赖此单例行的 `SELECT ... FOR UPDATE`，不依赖事务隔离级别下的空范围锁。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data target slot migration requires MySQL".into(),
            ));
        }
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE TABLE IF NOT EXISTS `biz_tenant_target_slot` (
                    `slot_id` TINYINT UNSIGNED NOT NULL,
                    `tenant_id` VARCHAR(64) DEFAULT NULL,
                    `placement_generation` BIGINT DEFAULT NULL,
                    `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
                    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
                    PRIMARY KEY (`slot_id`),
                    CONSTRAINT `ck_biz_tenant_target_slot_id` CHECK (`slot_id` = 1),
                    CONSTRAINT `ck_biz_tenant_target_slot_value` CHECK (
                        (`tenant_id` IS NULL AND `placement_generation` IS NULL AND `switch_token` IS NULL)
                        OR (`tenant_id` IS NOT NULL AND `placement_generation` > 0 AND `switch_token` IS NOT NULL)
                    )
                ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='dedicated 租户数据目标固定占用槽'"#,
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO `biz_tenant_target_slot` (`slot_id`, `updated_at`) \
                 VALUES (1, CURRENT_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE `slot_id` = VALUES(`slot_id`)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("biz_tenant_target_slot"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}
