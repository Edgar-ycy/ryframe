use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

const TENANT_TABLE: &str = "sys_tenant";
const CONFIG_TABLE: &str = "sys_config";

/// 增加租户配置版本、可迁移参数标记和配置包迁移记录。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("租户配置迁移要求 MySQL 8.0+".into()));
        }
        add_configuration_columns(manager).await?;
        ensure_tenant_scoped_file_key(manager.get_connection()).await?;
        create_transfer_tables(manager).await?;
        ensure_tenant_scoped_file_references(manager.get_connection()).await?;
        mark_builtin_portable_configs(manager.get_connection()).await?;
        seed_tenant_config_management_inner(manager.get_connection(), true).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "租户配置包和迁移历史属于前向业务数据，不能自动删除".into(),
        ))
    }
}

async fn add_configuration_columns(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if manager.has_table(TENANT_TABLE).await?
        && !manager
            .has_column(TENANT_TABLE, "configuration_version")
            .await?
    {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE `sys_tenant` \
                 ADD COLUMN `configuration_version` BIGINT NOT NULL DEFAULT 0 \
                 COMMENT '租户配置版本' AFTER `authorization_epoch`",
            )
            .await?;
    }
    if manager.has_table(CONFIG_TABLE).await?
        && !manager.has_column(CONFIG_TABLE, "portable").await?
    {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE `sys_config` \
                 ADD COLUMN `portable` TINYINT(1) NOT NULL DEFAULT 0 \
                 COMMENT '是否允许配置包迁移' AFTER `value`",
            )
            .await?;
    }
    Ok(())
}

async fn create_transfer_tables(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for statement in tenant_config_table_statements() {
        manager
            .get_connection()
            .execute_unprepared(statement)
            .await?;
    }
    Ok(())
}

pub(crate) fn tenant_config_table_statements() -> [&'static str; 4] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_bundle` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `origin` VARCHAR(16) NOT NULL,
            `source_tenant_key` VARCHAR(64) NOT NULL,
            `source_tenant_name_snapshot` VARCHAR(128) NOT NULL,
            `package_schema_version` VARCHAR(64) NOT NULL,
            `source_app_version` VARCHAR(32) NOT NULL,
            `file_id` BIGINT DEFAULT NULL,
            `sha256` CHAR(64) DEFAULT NULL,
            `resource_counts` JSON NOT NULL,
            `item_count` INT NOT NULL DEFAULT 0,
            `status` VARCHAR(24) NOT NULL DEFAULT 'pending',
            `background_job_id` BIGINT DEFAULT NULL,
            `idempotency_key_hash` CHAR(64) DEFAULT NULL,
            `created_by` BIGINT NOT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `expires_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_bundle_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_tenant_config_bundle_background_job` (`background_job_id`),
            UNIQUE KEY `uq_tenant_config_bundle_idempotency` (`tenant_id`, `created_by`, `idempotency_key_hash`),
            KEY `idx_tenant_config_bundle_list` (`tenant_id`, `created_at`, `id`),
            KEY `idx_tenant_config_bundle_expiry` (`status`, `expires_at`, `id`),
            KEY `idx_tenant_config_bundle_file` (`tenant_id`, `file_id`),
            CONSTRAINT `fk_tenant_config_bundle_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_bundle_file`
                FOREIGN KEY (`tenant_id`, `file_id`)
                REFERENCES `sys_file` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置包'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_transfer` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `bundle_id` BIGINT NOT NULL,
            `idempotency_key_hash` CHAR(64) NOT NULL,
            `request_kind` VARCHAR(24) NOT NULL,
            `request_fingerprint` CHAR(64) NOT NULL,
            `status` VARCHAR(24) NOT NULL DEFAULT 'preview_ready',
            `target_configuration_version` BIGINT NOT NULL,
            `target_authorization_epoch` INT NOT NULL,
            `plan_hash` CHAR(64) DEFAULT NULL,
            `preview_calculated_at` DATETIME(6) DEFAULT NULL,
            `preview_background_job_id` BIGINT DEFAULT NULL,
            `apply_background_job_id` BIGINT DEFAULT NULL,
            `rollback_background_job_id` BIGINT DEFAULT NULL,
            `snapshot_file_id` BIGINT DEFAULT NULL,
            `applied_configuration_version` BIGINT DEFAULT NULL,
            `applied_authorization_epoch` INT DEFAULT NULL,
            `change_counts` JSON NOT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `requested_by` BIGINT NOT NULL,
            `rollback_expires_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_transfer_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_tenant_config_transfer_idempotency` (`tenant_id`, `requested_by`, `idempotency_key_hash`),
            UNIQUE KEY `uq_tenant_config_transfer_preview_job` (`preview_background_job_id`),
            UNIQUE KEY `uq_tenant_config_transfer_apply_job` (`apply_background_job_id`),
            UNIQUE KEY `uq_tenant_config_transfer_rollback_job` (`rollback_background_job_id`),
            KEY `idx_tenant_config_transfer_list` (`tenant_id`, `created_at`, `id`),
            KEY `idx_tenant_config_transfer_status` (`tenant_id`, `status`, `created_at`),
            KEY `idx_tenant_config_transfer_bundle` (`tenant_id`, `bundle_id`),
            KEY `idx_tenant_config_transfer_snapshot` (`tenant_id`, `snapshot_file_id`),
            CONSTRAINT `fk_tenant_config_transfer_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_bundle`
                FOREIGN KEY (`tenant_id`, `bundle_id`)
                REFERENCES `sys_tenant_config_bundle` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_snapshot`
                FOREIGN KEY (`tenant_id`, `snapshot_file_id`)
                REFERENCES `sys_file` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置迁移'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_transfer_item` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `transfer_id` BIGINT NOT NULL,
            `resource_type` VARCHAR(32) NOT NULL,
            `stable_key` VARCHAR(384) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
            `display_name` VARCHAR(255) NOT NULL,
            `action` VARCHAR(16) NOT NULL,
            `outcome` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `detail_code` VARCHAR(64) DEFAULT NULL,
            `detail` VARCHAR(500) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_tenant_config_transfer_item` (`transfer_id`, `resource_type`, `stable_key`),
            KEY `idx_tenant_config_transfer_item_list` (`tenant_id`, `transfer_id`, `id`),
            KEY `idx_tenant_config_transfer_item_action` (`transfer_id`, `action`, `id`),
            CONSTRAINT `fk_tenant_config_transfer_item_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_tenant_config_transfer_item_transfer`
                FOREIGN KEY (`tenant_id`, `transfer_id`)
                REFERENCES `sys_tenant_config_transfer` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置迁移明细'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_tenant_config_lease` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `owner_token` VARCHAR(64) NOT NULL,
            `transfer_id` BIGINT NOT NULL,
            `operation` VARCHAR(16) NOT NULL,
            `expires_at` DATETIME(6) NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`tenant_id`),
            KEY `idx_tenant_config_lease_expiry` (`expires_at`, `tenant_id`),
            KEY `idx_tenant_config_lease_transfer` (`tenant_id`, `transfer_id`),
            CONSTRAINT `fk_tenant_config_lease_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_tenant_config_lease_transfer`
                FOREIGN KEY (`tenant_id`, `transfer_id`)
                REFERENCES `sys_tenant_config_transfer` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='租户配置迁移租约'"#,
    ]
}

async fn ensure_tenant_scoped_file_references<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    ensure_tenant_scoped_file_key(db).await?;
    ensure_tenant_scoped_file_reference(
        db,
        "sys_tenant_config_bundle",
        "file_id",
        "idx_tenant_config_bundle_file",
        "fk_tenant_config_bundle_file",
    )
    .await?;
    ensure_tenant_scoped_file_reference(
        db,
        "sys_tenant_config_transfer",
        "snapshot_file_id",
        "idx_tenant_config_transfer_snapshot",
        "fk_tenant_config_transfer_snapshot",
    )
    .await
}

async fn ensure_tenant_scoped_file_key<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    ensure_index(
        db,
        "sys_file",
        "uq_sys_file_tenant_id",
        true,
        &["tenant_id", "id"],
    )
    .await
}

async fn ensure_tenant_scoped_file_reference<C>(
    db: &C,
    table: &str,
    file_column: &str,
    index_name: &str,
    constraint_name: &str,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, table).await? {
        return Ok(());
    }
    let mismatch_sql = format!(
        "SELECT COUNT(*) FROM `{table}` child \
         JOIN `sys_file` file ON file.`id` = child.`{file_column}` \
         WHERE child.`{file_column}` IS NOT NULL \
           AND child.`tenant_id` <> file.`tenant_id`"
    );
    let mismatch_count = scalar_i64(db, &mismatch_sql).await?;
    if mismatch_count > 0 {
        return Err(DbErr::Custom(format!(
            "表 {table} 存在 {mismatch_count} 条跨租户文件引用，拒绝创建复合租户外键"
        )));
    }

    let expected_columns = ["tenant_id", file_column];
    let expected_referenced_columns = ["tenant_id", "id"];
    let foreign_key = foreign_key_definition(db, table, constraint_name).await?;
    let foreign_key_is_current = foreign_key.as_ref().is_some_and(|definition| {
        definition.columns == expected_columns
            && definition.referenced_table == "sys_file"
            && definition.referenced_columns == expected_referenced_columns
            && definition.update_rule == "CASCADE"
            && definition.delete_rule == "RESTRICT"
    });
    if foreign_key.is_some() && !foreign_key_is_current {
        let sql = format!("ALTER TABLE `{table}` DROP FOREIGN KEY `{constraint_name}`");
        db.execute_unprepared(&sql).await?;
    }

    ensure_index(db, table, index_name, false, &expected_columns).await?;

    if !foreign_key_is_current {
        let sql = format!(
            "ALTER TABLE `{table}` ADD CONSTRAINT `{constraint_name}` \
             FOREIGN KEY (`tenant_id`, `{file_column}`) \
             REFERENCES `sys_file` (`tenant_id`, `id`) \
             ON UPDATE CASCADE ON DELETE RESTRICT"
        );
        db.execute_unprepared(&sql).await?;
    }
    Ok(())
}

async fn ensure_index<C>(
    db: &C,
    table: &str,
    index_name: &str,
    unique: bool,
    expected_columns: &[&str],
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, table).await? {
        return Ok(());
    }
    if let Some(definition) = index_definition(db, table, index_name).await? {
        if definition.unique == unique && definition.columns == expected_columns {
            return Ok(());
        }
        let sql = format!("ALTER TABLE `{table}` DROP INDEX `{index_name}`");
        db.execute_unprepared(&sql).await?;
    }
    let unique_sql = if unique { "UNIQUE " } else { "" };
    let columns = expected_columns
        .iter()
        .map(|column| format!("`{column}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("ALTER TABLE `{table}` ADD {unique_sql}INDEX `{index_name}` ({columns})");
    db.execute_unprepared(&sql).await?;
    Ok(())
}

struct IndexDefinition {
    unique: bool,
    columns: Vec<String>,
}

async fn index_definition<C>(
    db: &C,
    table: &str,
    index_name: &str,
) -> Result<Option<IndexDefinition>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT CAST(NON_UNIQUE AS SIGNED), COLUMN_NAME \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND INDEX_NAME = ? \
             ORDER BY SEQ_IN_INDEX",
            [table.into(), index_name.into()],
        ))
        .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let unique = i64::try_get_by_index(&rows[0], 0)? == 0;
    let columns = rows
        .iter()
        .map(|row| String::try_get_by_index(row, 1))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(IndexDefinition { unique, columns }))
}

struct ForeignKeyDefinition {
    columns: Vec<String>,
    referenced_table: String,
    referenced_columns: Vec<String>,
    update_rule: String,
    delete_rule: String,
}

async fn foreign_key_definition<C>(
    db: &C,
    table: &str,
    constraint_name: &str,
) -> Result<Option<ForeignKeyDefinition>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT k.COLUMN_NAME, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, \
                    r.UPDATE_RULE, r.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
              AND r.TABLE_NAME = k.TABLE_NAME \
              AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
             WHERE k.CONSTRAINT_SCHEMA = DATABASE() \
               AND k.TABLE_NAME = ? AND k.CONSTRAINT_NAME = ? \
             ORDER BY k.ORDINAL_POSITION",
            [table.into(), constraint_name.into()],
        ))
        .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let referenced_table = String::try_get_by_index(&rows[0], 1)?;
    let update_rule = String::try_get_by_index(&rows[0], 3)?;
    let delete_rule = String::try_get_by_index(&rows[0], 4)?;
    let columns = rows
        .iter()
        .map(|row| String::try_get_by_index(row, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let referenced_columns = rows
        .iter()
        .map(|row| String::try_get_by_index(row, 2))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(ForeignKeyDefinition {
        columns,
        referenced_table,
        referenced_columns,
        update_rule,
        delete_rule,
    }))
}

async fn scalar_i64<C>(db: &C, sql: &str) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_string(DbBackend::MySql, sql.to_owned()))
        .await?
        .ok_or_else(|| DbErr::Custom("标量查询没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)?)
}

async fn mark_builtin_portable_configs<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, CONFIG_TABLE).await? {
        return Ok(());
    }
    db.execute_unprepared(
        "UPDATE `sys_config` SET `portable` = 1 \
         WHERE `key` IN ('sys.index.skinName', 'sys.index.sideTheme')",
    )
    .await?;
    Ok(())
}

/// 幂等补齐配置包迁移权限和菜单，不修改任何普通角色授权。
pub(crate) async fn seed_tenant_config_management<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    ensure_tenant_scoped_file_references(db).await?;
    seed_tenant_config_management_inner(db, false).await
}

async fn seed_tenant_config_management_inner<C>(
    db: &C,
    reject_preexisting_role_bindings: bool,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_permission").await? {
        return Ok(());
    }
    let tenants = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT tenant_id FROM sys_tenant ORDER BY id".to_owned(),
        ))
        .await?;
    for row in tenants {
        let tenant_id = String::try_get_by_index(&row, 0)?;
        let before = config_management_resource_count(db, &tenant_id).await?;
        for (code, name, sort) in [
            ("system:config-package:list", "配置包查询", 50),
            ("system:config-package:export", "配置包导出", 51),
            ("system:config-package:download", "配置包下载", 52),
            ("system:config-transfer:list", "配置迁移查询", 53),
            ("system:config-transfer:add", "配置迁移新增", 54),
            ("system:config-transfer:preview", "配置迁移预览", 55),
            ("system:config-transfer:apply", "配置迁移应用", 56),
            ("system:config-transfer:rollback", "配置迁移回滚", 57),
        ] {
            seed_permission(
                db,
                &tenant_id,
                code,
                name,
                sort,
                reject_preexisting_role_bindings,
            )
            .await?;
        }
        seed_menu(db, &tenant_id).await?;
        let after = config_management_resource_count(db, &tenant_id).await?;
        if after > before {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE sys_tenant SET configuration_version = configuration_version + 1, \
                 authorization_epoch = authorization_epoch + 1, updated_at = UTC_TIMESTAMP(6) \
                 WHERE tenant_id = ?",
                [tenant_id.into()],
            ))
            .await?;
        }
    }
    Ok(())
}

async fn config_management_resource_count<C>(db: &C, tenant_id: &str) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT \
             (SELECT COUNT(*) FROM sys_permission WHERE tenant_id = ? AND code IN (\
               'system:config-package:list','system:config-package:export','system:config-package:download',\
               'system:config-transfer:list','system:config-transfer:add','system:config-transfer:preview',\
               'system:config-transfer:apply','system:config-transfer:rollback')) + \
             (SELECT COUNT(*) FROM sys_menu WHERE tenant_id = ? \
               AND route_key = 'system.config-transfer' AND del_flag = '0')",
            [tenant_id.into(), tenant_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("配置迁移权限计数没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)?)
}

async fn seed_permission<C>(
    db: &C,
    tenant_id: &str,
    code: &str,
    name: &str,
    sort: i32,
    reject_preexisting_role_bindings: bool,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let parent_id = permission_id(db, tenant_id, "system")
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 缺少 system 父权限")))?;
    if let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, code, name, parent_id, perm_type, icon, sort, status \
             FROM sys_permission WHERE tenant_id = ? AND code = ? LIMIT 1",
            [tenant_id.into(), code.into()],
        ))
        .await?
    {
        let permission_id = i64::try_get_by_index(&row, 0)?;
        let actual_code = String::try_get_by_index(&row, 1)?;
        let actual_name = String::try_get_by_index(&row, 2)?;
        let actual_parent_id = Option::<i64>::try_get_by_index(&row, 3)?;
        let actual_type = String::try_get_by_index(&row, 4)?;
        let actual_icon = Option::<String>::try_get_by_index(&row, 5)?;
        let actual_sort = i32::try_get_by_index(&row, 6)?;
        let actual_status = String::try_get_by_index(&row, 7)?;
        if actual_code != code
            || actual_name != name
            || actual_parent_id != Some(parent_id)
            || actual_type != "api"
            || actual_icon.is_some()
            || actual_sort != sort
            || actual_status != "1"
        {
            return Err(DbErr::Custom(format!(
                "租户 {tenant_id} 的保留权限代码 {code} 已存在，但定义与配置迁移权限不一致"
            )));
        }
        if reject_preexisting_role_bindings
            && ordinary_role_has_permission(db, tenant_id, permission_id).await?
        {
            return Err(DbErr::Custom(format!(
                "租户 {tenant_id} 的保留权限代码 {code} 已绑定普通角色，拒绝将其接管为配置迁移权限"
            )));
        }
        return Ok(());
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_permission \
         (id, tenant_id, name, code, parent_id, perm_type, icon, sort, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            next_id()?.into(),
            tenant_id.into(),
            name.into(),
            code.into(),
            Some(parent_id).into(),
            sort.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn seed_menu<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let parent_id = menu_id(db, tenant_id, "system")
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 缺少 system 父菜单")))?;
    let permission_id = permission_id(db, tenant_id, "system:config-transfer:list")
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 缺少配置迁移查询权限")))?;
    let existing = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, del_flag \
             FROM sys_menu WHERE tenant_id = ? AND route_key = ? ORDER BY id LIMIT 2",
            [tenant_id.into(), "system.config-transfer".into()],
        ))
        .await?;
    if existing.len() > 1 {
        return Err(DbErr::Custom(format!(
            "租户 {tenant_id} 存在多个配置迁移 route_key 菜单"
        )));
    }
    if let Some(row) = existing.first() {
        let actual_name = String::try_get_by_index(row, 1)?;
        let actual_parent_id = Option::<i64>::try_get_by_index(row, 2)?;
        let actual_type = String::try_get_by_index(row, 3)?;
        let actual_permission_id = Option::<i64>::try_get_by_index(row, 4)?;
        let actual_route_key = Option::<String>::try_get_by_index(row, 5)?;
        let actual_icon = Option::<String>::try_get_by_index(row, 6)?;
        let actual_sort = i32::try_get_by_index(row, 7)?;
        let actual_visible = bool::try_get_by_index(row, 8)?;
        let actual_status = String::try_get_by_index(row, 9)?;
        let actual_del_flag = String::try_get_by_index(row, 10)?;
        if actual_name != "配置迁移"
            || actual_parent_id != Some(parent_id)
            || actual_type != "C"
            || actual_permission_id != Some(permission_id)
            || actual_route_key.as_deref() != Some("system.config-transfer")
            || actual_icon.as_deref() != Some("Switch")
            || actual_sort != 13
            || !actual_visible
            || actual_status != "1"
            || actual_del_flag != "0"
        {
            return Err(DbErr::Custom(format!(
                "租户 {tenant_id} 的保留 route_key system.config-transfer 已存在，但菜单定义不一致"
            )));
        }
        return Ok(());
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_menu \
         (id, tenant_id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, remark, del_flag, created_at, updated_at) \
         VALUES (?, ?, '配置迁移', ?, 'C', ?, 'system.config-transfer', 'Switch', 13, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            next_id()?.into(),
            tenant_id.into(),
            parent_id.into(),
            permission_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn ordinary_role_has_permission<C>(
    db: &C,
    tenant_id: &str,
    permission_id: i64,
) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT 1 FROM sys_role_permission relation \
             JOIN sys_role role ON role.tenant_id = relation.tenant_id AND role.id = relation.role_id \
             WHERE relation.tenant_id = ? AND relation.perm_id = ? \
             AND role.is_super = 0 AND role.del_flag = '0' LIMIT 1",
            [tenant_id.into(), permission_id.into()],
        ))
        .await?
        .is_some())
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

async fn menu_id<C>(db: &C, tenant_id: &str, route_key: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT id FROM sys_menu WHERE tenant_id = ? AND route_key = ? AND del_flag = '0' LIMIT 1",
        [tenant_id.into(), route_key.into()],
    ))
    .await?
    .map(|row| i64::try_get_by_index(&row, 0))
        .transpose()?)
}

async fn table_exists<C>(db: &C, table: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            [table.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("数据表存在性查询没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)? > 0)
}

fn next_id() -> Result<i64, DbErr> {
    ryframe_utils::snowflake::try_next_snowflake_id()
        .map_err(|error| DbErr::Custom(error.to_string()))
}
