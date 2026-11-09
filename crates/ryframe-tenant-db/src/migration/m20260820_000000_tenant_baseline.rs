use sea_orm::DbBackend;
use sea_orm_migration::prelude::*;

pub const TENANT_FENCE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `biz_tenant_fence` (
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户业务数据写入围栏'"#;

pub const TENANT_TARGET_SLOT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `biz_tenant_target_slot` (
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='dedicated 租户数据目标固定占用槽'"#;

pub const RESOURCE_OWNERSHIP_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `ryframe_resource_ownership` (
    `resource_kind` VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `scope_id` VARCHAR(48) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `marker` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (`resource_kind`),
    UNIQUE KEY `uq_resource_ownership_marker` (`marker`),
    UNIQUE KEY `uq_resource_ownership_scope` (`scope_id`, `resource_kind`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='物理数据库资源作用域所有权'"#;

pub const DATA_SCOPE_PROJECTION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `biz_data_scope_projection` (
    `tenant_id` VARCHAR(64) NOT NULL,
    `source_version` BIGINT NOT NULL,
    `build_token` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `placement_generation` BIGINT NOT NULL,
    `switch_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `digest` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `owner_count` BIGINT NOT NULL,
    `dept_count` BIGINT NOT NULL,
    `ready` TINYINT(1) NOT NULL,
    `published_at` DATETIME(6) DEFAULT NULL,
    `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    PRIMARY KEY (`tenant_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户数据范围投影发布状态'"#;

pub const DATA_SCOPE_OWNER_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `biz_data_scope_owner` (
    `tenant_id` VARCHAR(64) NOT NULL,
    `build_token` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `user_id` BIGINT NOT NULL,
    `dept_id` BIGINT DEFAULT NULL,
    PRIMARY KEY (`tenant_id`, `build_token`, `user_id`),
    KEY `idx_data_scope_owner_dept` (`tenant_id`, `build_token`, `dept_id`, `user_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户数据范围用户归属投影'"#;

pub const DATA_SCOPE_DEPT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS `biz_data_scope_dept` (
    `tenant_id` VARCHAR(64) NOT NULL,
    `build_token` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    `dept_id` BIGINT NOT NULL,
    `ancestors` VARCHAR(512) NOT NULL,
    PRIMARY KEY (`tenant_id`, `build_token`, `dept_id`),
    KEY `idx_data_scope_dept_ancestors` (`tenant_id`, `build_token`, `ancestors`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户数据范围部门树投影'"#;

pub(crate) const RESOURCE_OWNERSHIP_SCHEMA_DESCRIPTOR: &str = "v2|table=\"ryframe_resource_ownership\"|engine=\"innodb\"|charset=\"utf8mb4\"|collation=\"utf8mb4_general_ci\"|columns=[\"resource_kind\":\"varchar(32)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"PRI\":None:\"\":\"\";\"scope_id\":\"varchar(48)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"MUL\":None:\"\":\"\";\"marker\":\"varchar(128)\":\"NO\":Some(\"ascii\"):Some(\"ascii_bin\"):\"UNI\":None:\"\":\"\";\"created_at\":\"datetime(6)\":\"NO\":None:None:\"\":Some(\"current_timestamp(6)\"):\"\":\"\";\"updated_at\":\"datetime(6)\":\"NO\":None:None:\"\":Some(\"current_timestamp(6)\"):\"on update current_timestamp(6)\":\"\";]|indexes=[\"PRIMARY\":\"resource_kind\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_marker\":\"marker\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_scope\":\"scope_id\":1:0:\"btree\":None:\"YES\";\"uq_resource_ownership_scope\":\"resource_kind\":2:0:\"btree\":None:\"YES\";]|constraints=[\"PRIMARY\":\"PRIMARY KEY\":\"YES\";\"uq_resource_ownership_marker\":\"UNIQUE\":\"YES\";\"uq_resource_ownership_scope\":\"UNIQUE\":\"YES\";]|checks=[]|foreign_keys=[]";

pub(super) const TARGET_SLOT_SEED: &str = "INSERT INTO `biz_tenant_target_slot` (`slot_id`, `updated_at`) \
                 VALUES (1, CURRENT_TIMESTAMP(6)) \
                 ON DUPLICATE KEY UPDATE `slot_id` = `slot_id`";

pub(super) const SHARED_CONTROL_FENCE_SEED: &str = r#"INSERT INTO `biz_tenant_fence`
                        (`tenant_id`, `target_key`, `placement_generation`, `state`, `switch_token`, `updated_at`)
                       SELECT tenant.`tenant_id`, 'shared-control', 1, 'active',
                              COALESCE(placement.`switch_token`,
                                  SHA2(CONCAT('ryframe:tenant-data:shared-control:v1:', tenant.`tenant_id`), 256)),
                              CURRENT_TIMESTAMP(6)
                       FROM `sys_tenant` AS tenant
                       LEFT JOIN `sys_tenant_data_placement` AS placement
                         ON placement.`tenant_id` = tenant.`tenant_id`
                        AND placement.`current_target_key` = 'shared-control'
                        AND placement.`placement_generation` = 1
                        AND placement.`state` = 'active'
                       ON DUPLICATE KEY UPDATE
                         `tenant_id` = `biz_tenant_fence`.`tenant_id`"#;

pub(super) fn table_statements() -> [&'static str; 6] {
    [
        TENANT_FENCE_DDL,
        TENANT_TARGET_SLOT_DDL,
        RESOURCE_OWNERSHIP_DDL,
        DATA_SCOPE_PROJECTION_DDL,
        DATA_SCOPE_OWNER_DDL,
        DATA_SCOPE_DEPT_DDL,
    ]
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data baseline requires MySQL 8.0.16 or newer".into(),
            ));
        }

        let connection = manager.get_connection();
        for statement in table_statements() {
            connection.execute_unprepared(statement).await?;
        }
        connection.execute_unprepared(TARGET_SLOT_SEED).await?;

        // shared-control 拥有 sys_tenant；dedicated 目标保持空围栏，等待放置流程写入。
        if manager.has_table("sys_tenant").await? {
            connection
                .execute_unprepared(SHARED_CONTROL_FENCE_SEED)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "tenant-data baseline is destructive and cannot be rolled back".into(),
        ))
    }
}
