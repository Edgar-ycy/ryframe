use sea_orm::{ConnectionTrait, DbBackend};
use sea_orm_migration::prelude::*;

/// 安装租户数据放置、迁移和备份控制面。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data control migration requires MySQL 8.0.16 or newer".into(),
            ));
        }
        if !manager.has_table("sys_tenant").await? {
            return Err(DbErr::Custom(
                "sys_tenant is required before tenant-data control tables".into(),
            ));
        }

        for statement in tenant_data_control_table_statements() {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        seed_tenant_data_placements(manager.get_connection()).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "sys_tenant_data_migration_item",
            "sys_tenant_data_backup_point",
            "sys_tenant_data_migration",
            "sys_tenant_data_placement",
        ] {
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new(table))
                        .if_exists()
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

/// 租户数据控制面四张表的规范 DDL。
pub(crate) fn tenant_data_control_table_statements() -> [&'static str; 4] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_placement` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `current_target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `placement_generation` BIGINT NOT NULL,
            `state` VARCHAR(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_data_placement_target` (`current_target_key`, `state`, `tenant_id`),
            KEY `idx_tenant_data_placement_state` (`state`, `updated_at`),
            CONSTRAINT `fk_tenant_data_placement_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_placement_generation`
                CHECK (`placement_generation` > 0),
            CONSTRAINT `ck_tenant_data_placement_state`
                CHECK (`state` IN ('provisioning', 'active', 'maintenance', 'failed'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据权威放置'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_migration` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `source_target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_target_mode` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_target_kind` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_target_mode` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_target_kind` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `source_generation` BIGINT NOT NULL,
            `source_switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_generation` BIGINT NOT NULL,
            `source_schema_fingerprint` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `target_schema_fingerprint` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `plan_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `create_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `cancel_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `finalize_idempotency_key_hash` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `state` VARCHAR(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `operator_id` BIGINT NOT NULL,
            `cancelled_by` BIGINT DEFAULT NULL,
            `finalized_by` BIGINT DEFAULT NULL,
            `background_job_id` BIGINT DEFAULT NULL,
            `retention_hours` INT NOT NULL DEFAULT 168,
            `error_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_detail` VARCHAR(1000) DEFAULT NULL,
            `prechecked_at` DATETIME(6) DEFAULT NULL,
            `queued_at` DATETIME(6) DEFAULT NULL,
            `quiesced_at` DATETIME(6) DEFAULT NULL,
            `frozen_at` DATETIME(6) DEFAULT NULL,
            `copy_started_at` DATETIME(6) DEFAULT NULL,
            `copy_completed_at` DATETIME(6) DEFAULT NULL,
            `verified_at` DATETIME(6) DEFAULT NULL,
            `cut_over_at` DATETIME(6) DEFAULT NULL,
            `activated_at` DATETIME(6) DEFAULT NULL,
            `succeeded_at` DATETIME(6) DEFAULT NULL,
            `retention_until` DATETIME(6) DEFAULT NULL,
            `cancel_requested_at` DATETIME(6) DEFAULT NULL,
            `finalize_requested_at` DATETIME(6) DEFAULT NULL,
            `cleanup_ready_at` DATETIME(6) DEFAULT NULL,
            `finalized_at` DATETIME(6) DEFAULT NULL,
            `failed_at` DATETIME(6) DEFAULT NULL,
            `cancelled_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_migration_switch` (`switch_token`),
            UNIQUE KEY `uq_tenant_data_migration_create_key` (`create_idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_data_migration_cancel_key` (`cancel_idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_data_migration_finalize_key` (`finalize_idempotency_key_hash`),
            KEY `idx_tenant_data_migration_tenant` (`tenant_id`, `state`, `created_at`),
            KEY `idx_tenant_data_migration_target` (`target_key`, `state`, `created_at`),
            KEY `idx_tenant_data_migration_job` (`background_job_id`),
            CONSTRAINT `fk_tenant_data_migration_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_migration_generation`
                CHECK (`source_generation` > 0 AND `target_generation` > `source_generation`),
            CONSTRAINT `ck_tenant_data_migration_retention`
                CHECK (`retention_hours` BETWEEN 1 AND 8760),
            CONSTRAINT `ck_tenant_data_migration_target_contract`
                CHECK (`source_target_mode` IN ('shared', 'dedicated')
                    AND `target_target_mode` IN ('shared', 'dedicated')
                    AND `source_target_kind` IN ('control', 'mysql')
                    AND `target_target_kind` IN ('control', 'mysql')),
            CONSTRAINT `ck_tenant_data_migration_state`
                CHECK (`state` IN ('prechecking', 'queued', 'quiescing', 'frozen', 'copying', 'verifying', 'cutting_over', 'activating', 'succeeded', 'retention_pending', 'finalized', 'failed', 'cancelled'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据迁移任务'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_migration_item` (
            `id` BIGINT NOT NULL,
            `migration_id` BIGINT NOT NULL,
            `table_name` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `copy_order` INT NOT NULL,
            `state` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `cursor_json` JSON DEFAULT NULL,
            `source_row_count` BIGINT DEFAULT NULL,
            `target_row_count` BIGINT DEFAULT NULL,
            `source_digest` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `target_digest` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `error_detail` VARCHAR(1000) DEFAULT NULL,
            `copy_started_at` DATETIME(6) DEFAULT NULL,
            `copied_at` DATETIME(6) DEFAULT NULL,
            `verified_at` DATETIME(6) DEFAULT NULL,
            `cleanup_state` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
            `cleanup_row_count` BIGINT NOT NULL DEFAULT 0,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_migration_item` (`migration_id`, `table_name`),
            KEY `idx_tenant_data_migration_item_state` (`migration_id`, `state`, `copy_order`),
            CONSTRAINT `fk_tenant_data_migration_item_migration`
                FOREIGN KEY (`migration_id`) REFERENCES `sys_tenant_data_migration` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_data_migration_item_state`
                CHECK (`state` IN ('pending', 'copying', 'copied', 'verifying', 'verified', 'failed')),
            CONSTRAINT `ck_tenant_data_migration_item_cleanup_state`
                CHECK (`cleanup_state` IN ('pending', 'cleaning', 'cleaned')),
            CONSTRAINT `ck_tenant_data_migration_item_copy_order`
                CHECK (`copy_order` > 0),
            CONSTRAINT `ck_tenant_data_migration_item_counts`
                CHECK ((`source_row_count` IS NULL OR `source_row_count` >= 0)
                    AND (`target_row_count` IS NULL OR `target_row_count` >= 0)
                    AND `cleanup_row_count` >= 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据迁移表级检查点'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_data_backup_point` (
            `id` BIGINT NOT NULL,
            `scope` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `tenant_id` VARCHAR(64) DEFAULT NULL,
            `target_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `placement_generation` BIGINT DEFAULT NULL,
            `schema_fingerprint` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `provider_ref` VARCHAR(512) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `captured_at` DATETIME(6) NOT NULL,
            `checksum` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
            `validation_status` VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `validation_detail` VARCHAR(1000) DEFAULT NULL,
            `retention_until` DATETIME(6) NOT NULL,
            `expires_at` DATETIME(6) DEFAULT NULL,
            `last_restore_drill_at` DATETIME(6) DEFAULT NULL,
            `created_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_data_backup_provider_ref` (`provider_ref`),
            KEY `idx_tenant_data_backup_tenant` (`tenant_id`, `captured_at`),
            KEY `idx_tenant_data_backup_target` (`target_key`, `scope`, `captured_at`),
            KEY `idx_tenant_data_backup_expiry` (`expires_at`, `validation_status`),
            CONSTRAINT `fk_tenant_data_backup_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON DELETE RESTRICT,
            CONSTRAINT `ck_tenant_data_backup_scope`
                CHECK ((`scope` = 'tenant' AND `tenant_id` IS NOT NULL AND `placement_generation` > 0)
                    OR (`scope` = 'shard' AND `tenant_id` IS NULL)),
            CONSTRAINT `ck_tenant_data_backup_validation`
                CHECK (`validation_status` IN ('pending', 'valid', 'invalid')),
            CONSTRAINT `ck_tenant_data_backup_retention`
                CHECK (`expires_at` IS NULL OR `expires_at` >= `retention_until`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据备份恢复点'"#,
    ]
}

pub(crate) async fn seed_tenant_data_placements<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    db.execute_unprepared(
        r#"INSERT INTO `sys_tenant_data_placement`
            (`tenant_id`, `current_target_key`, `placement_generation`, `state`, `switch_token`,
             `created_at`, `updated_at`)
           SELECT tenant.`tenant_id`, 'shared-control', 1,
                  'active',
                  SHA2(CONCAT('ryframe:tenant-data:shared-control:v1:', tenant.`tenant_id`), 256),
                  CURRENT_TIMESTAMP(6), CURRENT_TIMESTAMP(6)
           FROM `sys_tenant` AS tenant
           ON DUPLICATE KEY UPDATE
             `switch_token` = IF(
               `current_target_key` = 'shared-control'
               AND `placement_generation` = 1
               AND `state` = 'active',
               VALUES(`switch_token`),
               `switch_token`
             )"#,
    )
    .await?;
    Ok(())
}
