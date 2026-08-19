use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::prelude::*;

/// 建立租户缓存命名空间的数据库权威版本，并为现有租户初始化配置命名空间。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::MySql {
            return Err(DbErr::Custom("缓存命名空间版本仅支持 MySQL".into()));
        }
        manager
            .get_connection()
            .execute_unprepared(CACHE_NAMESPACE_VERSION_DDL)
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO `sys_cache_namespace_version` \
                 (`tenant_id`, `namespace`, `version`, `created_at`, `updated_at`) \
                 SELECT `tenant_id`, 'config', 0, UTC_TIMESTAMP(), UTC_TIMESTAMP() \
                 FROM `sys_tenant` \
                 ON DUPLICATE KEY UPDATE `tenant_id` = VALUES(`tenant_id`)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "缓存命名空间版本是失效协议的权威状态，不能自动回滚删除".into(),
        ))
    }
}

const CACHE_NAMESPACE_VERSION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `sys_cache_namespace_version` (
    `tenant_id` VARCHAR(64) NOT NULL COMMENT '租户标识',
    `namespace` VARCHAR(64) NOT NULL COMMENT '缓存命名空间',
    `version` BIGINT NOT NULL DEFAULT 0 COMMENT '单调递增权威版本',
    `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    `updated_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    PRIMARY KEY (`tenant_id`, `namespace`),
    CONSTRAINT `fk_cache_namespace_version_tenant`
        FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
        ON UPDATE CASCADE ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户缓存命名空间权威版本'"#;
