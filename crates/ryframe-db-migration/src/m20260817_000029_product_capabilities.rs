use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

const SYSTEM_TENANT_ID: &str = "system";
const STANDARD_PLAN_ID: i64 = 1;
const PLATFORM_PLAN_ID: i64 = 2;
const STANDARD_VERSION_ID: i64 = 1;
const PLATFORM_VERSION_ID: i64 = 2;
const SERVICE_ACCOUNTS_CAPABILITY: &str = "system.service_accounts";
const PRODUCT_PERMISSIONS: &[(&str, &str, i32)] = &[
    ("platform:product-plan:list", "产品套餐查询", 6),
    ("platform:product-plan:add", "产品套餐新增", 7),
    ("platform:product-plan:edit", "产品套餐修改", 8),
    ("platform:product-plan:publish", "产品套餐版本发布", 9),
    ("tenant:product:view", "租户产品上下文查看", 10),
    ("tenant:product:assign", "租户产品套餐分配", 11),
    ("tenant:capability:override", "租户能力覆盖", 12),
    ("tenant:data-placement:view", "租户数据放置查看", 13),
    ("tenant:data-migration:list", "租户数据迁移查询", 14),
    ("tenant:data-migration:create", "租户数据迁移创建", 15),
    ("tenant:data-migration:cancel", "租户数据迁移取消", 16),
    ("tenant:data-migration:finalize", "租户数据迁移完成", 17),
    ("tenant:data-backup:list", "租户数据备份查询", 18),
];

/// 安装产品套餐、编译期能力映射、租户覆盖、运行时纪元和跨操作统一租约。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("产品能力迁移要求 MySQL 8.4".into()));
        }
        if !manager.has_table("sys_tenant").await? {
            return Err(DbErr::Custom(
                "缺少 sys_tenant，无法安装产品能力模型".into(),
            ));
        }
        if !manager.has_column("sys_tenant", "runtime_epoch").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE `sys_tenant` ADD COLUMN `runtime_epoch` BIGINT NOT NULL DEFAULT 1 COMMENT '租户运行时产品上下文纪元' AFTER `authorization_epoch`",
                )
                .await?;
        }
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE `sys_tenant` MODIFY COLUMN `status` VARCHAR(32) NOT NULL DEFAULT 'enabled' COMMENT '生命周期状态'",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE `sys_tenant` SET `status` = CASE `status` WHEN '1' THEN 'enabled' WHEN '0' THEN 'disabled' ELSE `status` END",
            )
            .await?;
        for statement in product_capability_table_statements() {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        migrate_config_operation_leases(manager).await?;
        seed_product_capabilities_inner(manager.get_connection()).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "产品分配、能力覆盖与运行时纪元属于前向业务数据，不能自动删除".into(),
        ))
    }
}

/// 当前版本产品能力与租户创建 Saga 相关七张表的规范 DDL。
pub(crate) fn product_capability_table_statements() -> [&'static str; 7] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan` (
            `id` BIGINT NOT NULL,
            `plan_key` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `status` CHAR(1) NOT NULL DEFAULT '1',
            `created_by` BIGINT NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_product_plan_key` (`plan_key`),
            KEY `idx_product_plan_status` (`status`, `plan_key`),
            CONSTRAINT `ck_product_plan_status` CHECK (`status` IN ('0', '1')),
            CONSTRAINT `ck_product_plan_key` CHECK (CHAR_LENGTH(`plan_key`) > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan_version` (
            `id` BIGINT NOT NULL,
            `plan_id` BIGINT NOT NULL,
            `version` INT NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'draft',
            `created_by` BIGINT NOT NULL,
            `published_by` BIGINT DEFAULT NULL,
            `published_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_product_plan_version` (`plan_id`, `version`),
            KEY `idx_product_plan_version_status` (`plan_id`, `status`, `version`),
            CONSTRAINT `fk_product_plan_version_plan`
                FOREIGN KEY (`plan_id`) REFERENCES `sys_product_plan` (`id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_product_plan_version_status`
                CHECK (`status` IN ('draft', 'published', 'retired'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐版本'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_product_plan_capability` (
            `plan_version_id` BIGINT NOT NULL,
            `capability_code` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `variant_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `schema_version` INT NOT NULL,
            `config` JSON NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`plan_version_id`, `capability_code`),
            KEY `idx_product_plan_capability_code` (`capability_code`, `plan_version_id`),
            CONSTRAINT `fk_product_plan_capability_version`
                FOREIGN KEY (`plan_version_id`) REFERENCES `sys_product_plan_version` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_product_plan_capability_code`
                CHECK (CHAR_LENGTH(`capability_code`) > 0),
            CONSTRAINT `ck_product_plan_capability_variant`
                CHECK (CHAR_LENGTH(`variant_code`) > 0),
            CONSTRAINT `ck_product_plan_capability_schema`
                CHECK (`schema_version` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='产品套餐版本能力'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_product_plan` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `plan_version_id` BIGINT NOT NULL,
            `changed_by` BIGINT DEFAULT NULL,
            `change_reason` VARCHAR(500) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_product_plan_version` (`plan_version_id`, `tenant_id`),
            CONSTRAINT `fk_tenant_product_plan_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_tenant_product_plan_version`
                FOREIGN KEY (`plan_version_id`) REFERENCES `sys_product_plan_version` (`id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户产品套餐分配'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_capability_override` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `capability_code` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `enabled` TINYINT(1) NOT NULL,
            `variant_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `schema_version` INT NOT NULL,
            `config` JSON NOT NULL,
            `reason` VARCHAR(500) DEFAULT NULL,
            `changed_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`, `capability_code`),
            KEY `idx_tenant_capability_override_code` (`capability_code`, `tenant_id`),
            CONSTRAINT `fk_tenant_capability_override_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_capability_override_enabled`
                CHECK (`enabled` IN (0, 1)),
            CONSTRAINT `ck_tenant_capability_override_code`
                CHECK (CHAR_LENGTH(`capability_code`) > 0),
            CONSTRAINT `ck_tenant_capability_override_variant`
                CHECK (CHAR_LENGTH(`variant_code`) > 0),
            CONSTRAINT `ck_tenant_capability_override_schema`
                CHECK (`schema_version` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户能力覆盖'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_provision_request` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `request_token` CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `admin_password_hash` VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            CONSTRAINT `fk_tenant_provision_request_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `ck_tenant_provision_request_token`
                CHECK (CHAR_LENGTH(`request_token`) = 64)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户创建 Saga 权威幂等请求'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_operation_lease` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `owner_token` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `operation` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `resource_type` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `resource_id` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `expires_at` DATETIME(6) NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_operation_lease_expiry` (`expires_at`, `tenant_id`),
            KEY `idx_tenant_operation_lease_resource` (`tenant_id`, `resource_type`, `resource_id`),
            CONSTRAINT `fk_tenant_operation_lease_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户统一操作租约'"#,
    ]
}

async fn migrate_config_operation_leases(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_tenant_config_lease").await? {
        return Ok(());
    }
    // 迁移期间不会运行后台 Worker。仅转换仍有效的租约；过期租约显式清除，避免升级后
    // 无意义地阻塞新的租户操作。
    manager
        .get_connection()
        .execute_unprepared(
            "INSERT INTO `sys_tenant_operation_lease` \
             (`tenant_id`, `owner_token`, `operation`, `resource_type`, `resource_id`, `expires_at`, `created_at`, `updated_at`) \
             SELECT `tenant_id`, `owner_token`, CONCAT('tenant_config.', `operation`), \
                    'tenant_config_transfer', CAST(`transfer_id` AS CHAR), `expires_at`, `created_at`, `updated_at` \
             FROM `sys_tenant_config_lease` WHERE `expires_at` > UTC_TIMESTAMP(6) \
             ON DUPLICATE KEY UPDATE `owner_token` = VALUES(`owner_token`), \
                 `operation` = VALUES(`operation`), `resource_type` = VALUES(`resource_type`), \
                 `resource_id` = VALUES(`resource_id`), `expires_at` = VALUES(`expires_at`), \
                 `updated_at` = VALUES(`updated_at`)",
        )
        .await?;
    manager
        .get_connection()
        .execute_unprepared("DROP TABLE `sys_tenant_config_lease`")
        .await?;
    Ok(())
}

pub(crate) async fn seed_product_capabilities<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    seed_product_capabilities_inner(db).await
}

async fn seed_product_capabilities_inner<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    seed_builtin_plans(db).await?;
    map_existing_tenants(db).await?;
    seed_product_permissions(db).await?;
    seed_product_menu(db).await
}

async fn seed_builtin_plans<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    db.execute_unprepared(&format!(
        "INSERT INTO `sys_product_plan` (`id`, `plan_key`, `name`, `description`, `status`, `created_by`, `created_at`, `updated_at`) VALUES \
         ({STANDARD_PLAN_ID}, 'standard', '标准版', '普通租户的默认产品套餐', '1', 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         ({PLATFORM_PLAN_ID}, 'platform', '平台版', '系统租户的平台控制面套餐', '1', 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `id` = `id`"
    ))
    .await?;
    db.execute_unprepared(&format!(
        "INSERT INTO `sys_product_plan_version` (`id`, `plan_id`, `version`, `name`, `description`, `status`, `created_by`, `published_by`, `published_at`, `created_at`, `updated_at`) VALUES \
         ({STANDARD_VERSION_ID}, {STANDARD_PLAN_ID}, 1, '标准版 v1', '标准版初始能力集合', 'published', 1, 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         ({PLATFORM_VERSION_ID}, {PLATFORM_PLAN_ID}, 1, '平台版 v1', '平台控制面初始能力集合', 'published', 1, 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `id` = `id`"
    ))
    .await?;
    db.execute_unprepared(&format!(
        "INSERT INTO `sys_product_plan_capability` (`plan_version_id`, `capability_code`, `variant_code`, `schema_version`, `config`, `created_at`, `updated_at`) VALUES \
         ({PLATFORM_VERSION_ID}, '{SERVICE_ACCOUNTS_CAPABILITY}', 'default', 1, JSON_OBJECT(), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)) \
         ON DUPLICATE KEY UPDATE `plan_version_id` = `plan_version_id`"
    ))
    .await?;
    validate_builtin_plan(db, STANDARD_PLAN_ID, "standard", STANDARD_VERSION_ID, false).await?;
    validate_builtin_plan(db, PLATFORM_PLAN_ID, "platform", PLATFORM_VERSION_ID, true).await
}

async fn validate_builtin_plan<C>(
    db: &C,
    plan_id: i64,
    plan_key: &str,
    version_id: i64,
    capability_enabled: bool,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT p.id, v.id, v.version, c.capability_code FROM sys_product_plan p \
             JOIN sys_product_plan_version v ON v.plan_id = p.id \
             LEFT JOIN sys_product_plan_capability c ON c.plan_version_id = v.id AND c.capability_code = ? \
             WHERE p.plan_key = ? LIMIT 1",
            [SERVICE_ACCOUNTS_CAPABILITY.into(), plan_key.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom(format!("内置产品套餐 {plan_key} v1 未正确创建")))?;
    let actual_plan_id = i64::try_get_by_index(&row, 0)?;
    let actual_version_id = i64::try_get_by_index(&row, 1)?;
    let actual_version = i32::try_get_by_index(&row, 2)?;
    let capability_row = Option::<String>::try_get_by_index(&row, 3)?;
    if (
        actual_plan_id,
        actual_version_id,
        actual_version,
        capability_row,
    ) != (
        plan_id,
        version_id,
        1,
        capability_enabled.then(|| SERVICE_ACCOUNTS_CAPABILITY.to_owned()),
    ) {
        return Err(DbErr::Custom(format!(
            "内置产品套餐 {plan_key} 与保留定义冲突，拒绝覆盖现有数据"
        )));
    }
    Ok(())
}

async fn map_existing_tenants<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_tenant").await? {
        return Ok(());
    }
    db.execute_unprepared(&format!(
        "INSERT INTO `sys_tenant_product_plan` \
         (`tenant_id`, `plan_version_id`, `changed_by`, `change_reason`, `created_at`, `updated_at`) \
         SELECT t.`tenant_id`, IF(t.`tenant_id` = '{SYSTEM_TENANT_ID}', {PLATFORM_VERSION_ID}, {STANDARD_VERSION_ID}), \
                NULL, 'initial_migration', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) \
         FROM `sys_tenant` t LEFT JOIN `sys_tenant_product_plan` tp ON tp.`tenant_id` = t.`tenant_id` \
         WHERE tp.`tenant_id` IS NULL"
    ))
    .await?;

    // 标准版默认关闭服务账号。只为迁移前已经真实使用服务账号，或已把相关治理权限
    // 授予角色的普通租户保留旧行为，避免把新能力意外开放给所有租户。
    db.execute_unprepared(&format!(
        "INSERT INTO `sys_tenant_capability_override` \
         (`tenant_id`, `capability_code`, `enabled`, `variant_code`, `schema_version`, `config`, `reason`, `changed_by`, `created_at`, `updated_at`) \
         SELECT t.`tenant_id`, '{SERVICE_ACCOUNTS_CAPABILITY}', 1, 'default', 1, JSON_OBJECT(), 'migration_existing_usage', NULL, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6) \
         FROM `sys_tenant` t \
         WHERE t.`tenant_id` <> '{SYSTEM_TENANT_ID}' AND ( \
             EXISTS (SELECT 1 FROM `sys_service_account` a WHERE a.`tenant_id` = t.`tenant_id` LIMIT 1) OR \
             EXISTS (SELECT 1 FROM `sys_service_delegation` d WHERE d.`tenant_id` = t.`tenant_id` LIMIT 1) OR \
             EXISTS (SELECT 1 FROM `sys_service_access_audit` a WHERE a.`tenant_id` = t.`tenant_id` LIMIT 1) OR \
             EXISTS (SELECT 1 FROM `sys_role_permission` rp \
                     JOIN `sys_permission` p ON p.`tenant_id` = rp.`tenant_id` AND p.`id` = rp.`perm_id` \
                     WHERE rp.`tenant_id` = t.`tenant_id` AND ( \
                         p.`code` LIKE 'system:service-account:%' OR \
                         p.`code` LIKE 'system:service-delegation:%' OR \
                         p.`code` LIKE 'system:service-access-audit:%') LIMIT 1)) \
         ON DUPLICATE KEY UPDATE `reason` = `sys_tenant_capability_override`.`reason`"
    ))
    .await?;

    // 000027 为所有当时存在的租户创建了服务账号菜单与权限。产品模型上线后，
    // 标准版未开通租户必须把这些历史资源休眠，关系保留以便以后受控恢复。
    db.execute_unprepared(&format!(
        "UPDATE `sys_permission` p SET p.`status` = IF(\
             COALESCE((SELECT o.`enabled` FROM `sys_tenant_capability_override` o \
                       WHERE o.`tenant_id` = p.`tenant_id` AND o.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}' LIMIT 1), \
                      (SELECT COUNT(*) > 0 FROM `sys_tenant_product_plan` tp \
                       JOIN `sys_product_plan_capability` pc ON pc.`plan_version_id` = tp.`plan_version_id` \
                       WHERE tp.`tenant_id` = p.`tenant_id` AND pc.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}')),
             '1', '0') \
         WHERE p.`tenant_id` <> '{SYSTEM_TENANT_ID}' AND (\
             p.`code` LIKE 'system:service-account:%' OR \
             p.`code` LIKE 'system:service-delegation:%' OR \
             p.`code` LIKE 'system:service-access-audit:%')"
    ))
    .await?;
    db.execute_unprepared(&format!(
        "UPDATE `sys_menu` m SET \
             m.`status` = IF(COALESCE((SELECT o.`enabled` FROM `sys_tenant_capability_override` o \
                                      WHERE o.`tenant_id` = m.`tenant_id` AND o.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}' LIMIT 1), \
                                     (SELECT COUNT(*) > 0 FROM `sys_tenant_product_plan` tp \
                                      JOIN `sys_product_plan_capability` pc ON pc.`plan_version_id` = tp.`plan_version_id` \
                                      WHERE tp.`tenant_id` = m.`tenant_id` AND pc.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}')), '1', '0'), \
             m.`visible` = IF(COALESCE((SELECT o.`enabled` FROM `sys_tenant_capability_override` o \
                                       WHERE o.`tenant_id` = m.`tenant_id` AND o.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}' LIMIT 1), \
                                      (SELECT COUNT(*) > 0 FROM `sys_tenant_product_plan` tp \
                                       JOIN `sys_product_plan_capability` pc ON pc.`plan_version_id` = tp.`plan_version_id` \
                                       WHERE tp.`tenant_id` = m.`tenant_id` AND pc.`capability_code` = '{SERVICE_ACCOUNTS_CAPABILITY}')), 1, 0) \
         WHERE m.`tenant_id` <> '{SYSTEM_TENANT_ID}' AND m.`route_key` = 'system.service-accounts'"
    ))
    .await?;
    Ok(())
}

async fn seed_product_permissions<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !tenant_exists(db, SYSTEM_TENANT_ID).await? {
        return Ok(());
    }
    let parent_id = permission_id(db, SYSTEM_TENANT_ID, "tenant:manage")
        .await?
        .ok_or_else(|| DbErr::Custom("系统租户缺少 tenant:manage 父权限".into()))?;
    for (code, name, sort) in PRODUCT_PERMISSIONS {
        if db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "SELECT tenant_id FROM sys_permission WHERE tenant_id <> ? AND code = ? LIMIT 1",
                [SYSTEM_TENANT_ID.into(), (*code).into()],
            ))
            .await?
            .is_some()
        {
            return Err(DbErr::Custom(format!(
                "普通租户存在平台保留权限代码 {code}，请先完成权限冲突治理"
            )));
        }
        if let Some(row) = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "SELECT name, parent_id, perm_type, icon, sort, status FROM sys_permission \
                 WHERE tenant_id = ? AND code = ? LIMIT 1",
                [SYSTEM_TENANT_ID.into(), (*code).into()],
            ))
            .await?
        {
            let definition = (
                String::try_get_by_index(&row, 0)?,
                Option::<i64>::try_get_by_index(&row, 1)?,
                String::try_get_by_index(&row, 2)?,
                Option::<String>::try_get_by_index(&row, 3)?,
                i32::try_get_by_index(&row, 4)?,
                String::try_get_by_index(&row, 5)?,
            );
            if definition
                != (
                    (*name).to_owned(),
                    Some(parent_id),
                    "api".to_owned(),
                    None,
                    *sort,
                    "1".to_owned(),
                )
            {
                return Err(DbErr::Custom(format!(
                    "系统租户的保留权限代码 {code} 与产品控制面定义冲突"
                )));
            }
            continue;
        }
        let id = ryframe_utils::snowflake::try_next_snowflake_id()
            .map_err(|error| DbErr::Custom(error.to_string()))?;
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_permission \
             (id, tenant_id, name, code, parent_id, perm_type, icon, sort, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [
                id.into(),
                SYSTEM_TENANT_ID.into(),
                (*name).into(),
                (*code).into(),
                parent_id.into(),
                (*sort).into(),
            ],
        ))
        .await?;
        db.execute_unprepared(
            "UPDATE sys_tenant SET configuration_version = configuration_version + 1, \
             authorization_epoch = authorization_epoch + 1, updated_at = UTC_TIMESTAMP(6) \
             WHERE tenant_id = 'system'",
        )
        .await?;
    }
    Ok(())
}

async fn seed_product_menu<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !tenant_exists(db, SYSTEM_TENANT_ID).await? {
        return Ok(());
    }
    let platform_id = ensure_menu(
        db,
        "platform",
        "平台管理",
        None,
        "M",
        None,
        Some("OfficeBuilding"),
        4,
    )
    .await?;
    let tenant_permission = permission_id(db, SYSTEM_TENANT_ID, "tenant:list")
        .await?
        .ok_or_else(|| DbErr::Custom("系统租户缺少 tenant:list 权限".into()))?;
    ensure_menu(
        db,
        "platform.tenant",
        "租户管理",
        Some(platform_id),
        "C",
        Some(tenant_permission),
        Some("OfficeBuilding"),
        1,
    )
    .await?;
    let product_permission = permission_id(db, SYSTEM_TENANT_ID, "platform:product-plan:list")
        .await?
        .ok_or_else(|| DbErr::Custom("系统租户缺少 platform:product-plan:list 权限".into()))?;
    ensure_menu(
        db,
        "platform.product-plans",
        "产品套餐",
        Some(platform_id),
        "C",
        Some(product_permission),
        Some("Box"),
        2,
    )
    .await?;
    let data_target_permission = permission_id(db, SYSTEM_TENANT_ID, "tenant:data-placement:view")
        .await?
        .ok_or_else(|| DbErr::Custom("系统租户缺少 tenant:data-placement:view 权限".into()))?;
    ensure_menu(
        db,
        "platform.data-targets",
        "数据目标",
        Some(platform_id),
        "C",
        Some(data_target_permission),
        Some("DataLine"),
        3,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn ensure_menu<C>(
    db: &C,
    route_key: &str,
    name: &str,
    parent_id: Option<i64>,
    menu_type: &str,
    permission_id: Option<i64>,
    icon: Option<&str>,
    sort: i32,
) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, name, parent_id, menu_type, perm_id, icon, sort, visible, status, del_flag \
             FROM sys_menu WHERE tenant_id = ? AND route_key = ? LIMIT 1",
            [SYSTEM_TENANT_ID.into(), route_key.into()],
        ))
        .await?
    {
        let id = i64::try_get_by_index(&row, 0)?;
        let actual = (
            String::try_get_by_index(&row, 1)?,
            Option::<i64>::try_get_by_index(&row, 2)?,
            String::try_get_by_index(&row, 3)?,
            Option::<i64>::try_get_by_index(&row, 4)?,
            Option::<String>::try_get_by_index(&row, 5)?,
            i32::try_get_by_index(&row, 6)?,
            bool::try_get_by_index(&row, 7)?,
            String::try_get_by_index(&row, 8)?,
            String::try_get_by_index(&row, 9)?,
        );
        let expected = (
            name.to_owned(),
            parent_id,
            menu_type.to_owned(),
            permission_id,
            icon.map(str::to_owned),
            sort,
            true,
            "1".to_owned(),
            "0".to_owned(),
        );
        if actual != expected {
            return Err(DbErr::Custom(format!(
                "系统租户保留菜单 {route_key} 与产品控制面定义冲突"
            )));
        }
        return Ok(id);
    }
    let id = ryframe_utils::snowflake::try_next_snowflake_id()
        .map_err(|error| DbErr::Custom(error.to_string()))?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_menu \
         (id, tenant_id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, del_flag, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, '1', '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            id.into(),
            SYSTEM_TENANT_ID.into(),
            name.into(),
            parent_id.into(),
            menu_type.into(),
            permission_id.into(),
            route_key.into(),
            icon.into(),
            sort.into(),
        ],
    ))
    .await?;
    Ok(id)
}

async fn permission_id<C>(db: &C, tenant_id: &str, code: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_permission WHERE tenant_id = ? AND code = ? LIMIT 1",
            [tenant_id.into(), code.into()],
        ))
        .await?
        .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?)
}

async fn tenant_exists<C>(db: &C, tenant_id: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT 1 FROM sys_tenant WHERE tenant_id = ? LIMIT 1",
            [tenant_id.into()],
        ))
        .await?
        .is_some())
}

async fn table_exists<C>(db: &C, table: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?",
            [table.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("表存在性检查没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)? > 0)
}
