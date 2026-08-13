use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;
use std::borrow::Cow;

const SERVICE_ACCOUNT_PERMISSIONS: &[(&str, &str, i32)] = &[
    ("system:service-account:list", "服务账号查询", 60),
    ("system:service-account:add", "服务账号新增", 61),
    ("system:service-account:edit", "服务账号修改", 62),
    ("system:service-account:remove", "服务账号删除", 63),
    ("system:service-account:role", "服务账号角色授权", 64),
    ("system:service-account:key-rotate", "服务账号 Key 轮换", 65),
    ("system:service-account:key-revoke", "服务账号 Key 撤销", 66),
    ("system:service-delegation:list", "服务委托查询", 67),
    ("system:service-delegation:revoke", "服务委托撤销", 68),
    ("system:service-access-audit:list", "服务访问审计查询", 69),
];

/// 安装服务账号、API Key、用户委托和访问审计的持久化底座。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("服务账号迁移要求 MySQL 8.0+".into()));
        }
        ensure_tenant_identity_indexes(manager).await?;
        for statement in service_account_table_statements() {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        seed_service_account_management_inner(manager.get_connection(), true).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "服务账号、凭据、委托和访问审计属于前向安全数据，不能自动删除".into(),
        ))
    }
}

async fn ensure_tenant_identity_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for (table, name) in [
        ("sys_dept", "uq_sys_dept_tenant_id"),
        ("sys_user", "uq_sys_user_tenant_id"),
        ("sys_role", "uq_sys_role_tenant_id"),
    ] {
        if !manager.has_index(table, name).await? {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE `{table}` ADD UNIQUE KEY `{name}` (`tenant_id`, `id`)"
                ))
                .await?;
        }
    }
    Ok(())
}

/// 当前版本服务账号相关六张表的规范 DDL。
pub(crate) fn service_account_table_statements() -> [&'static str; 6] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_service_account` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `code` VARCHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
            `name` VARCHAR(128) NOT NULL,
            `description` VARCHAR(500) DEFAULT NULL,
            `dept_id` BIGINT DEFAULT NULL,
            `status` CHAR(1) NOT NULL DEFAULT '1',
            `authorization_version` INT NOT NULL DEFAULT 1,
            `max_requests_per_minute` INT NOT NULL DEFAULT 60,
            `created_by` BIGINT NOT NULL,
            `del_flag` CHAR(1) NOT NULL DEFAULT '0',
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_account_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_service_account_code` (`tenant_id`, `code`),
            KEY `idx_service_account_list` (`tenant_id`, `del_flag`, `created_at`, `id`),
            KEY `idx_service_account_dept` (`tenant_id`, `dept_id`),
            KEY `fk_service_account_creator` (`tenant_id`, `created_by`),
            CONSTRAINT `fk_service_account_tenant`
                FOREIGN KEY (`tenant_id`) REFERENCES `sys_tenant` (`tenant_id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_account_dept`
                FOREIGN KEY (`tenant_id`, `dept_id`) REFERENCES `sys_dept` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_account_creator`
                FOREIGN KEY (`tenant_id`, `created_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_account_status` CHECK (`status` IN ('0', '1')),
            CONSTRAINT `ck_service_account_del_flag` CHECK (`del_flag` IN ('0', '2')),
            CONSTRAINT `ck_service_account_rate` CHECK (`max_requests_per_minute` > 0)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_account_role` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `role_id` BIGINT NOT NULL,
            PRIMARY KEY (`tenant_id`, `account_id`, `role_id`),
            KEY `idx_service_account_role_role` (`tenant_id`, `role_id`, `account_id`),
            CONSTRAINT `fk_service_account_role_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_account_role_role`
                FOREIGN KEY (`tenant_id`, `role_id`) REFERENCES `sys_role` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号角色关系'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_credential` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `key_id` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `secret_mac` BINARY(32) NOT NULL,
            `pepper_version` INT NOT NULL,
            `label` VARCHAR(128) NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'active',
            `expires_at` DATETIME(6) NOT NULL,
            `last_used_at` DATETIME(6) DEFAULT NULL,
            `created_by` BIGINT NOT NULL,
            `revoked_at` DATETIME(6) DEFAULT NULL,
            `revoked_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            `idempotency_key_hash` BINARY(32) NOT NULL,
            `request_fingerprint` BINARY(32) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_credential_key_id` (`key_id`),
            UNIQUE KEY `uq_service_credential_idempotency` (`tenant_id`, `account_id`, `idempotency_key_hash`),
            KEY `idx_service_credential_active` (`tenant_id`, `account_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_credential_expiry` (`status`, `expires_at`, `id`),
            KEY `fk_service_credential_creator` (`tenant_id`, `created_by`),
            KEY `fk_service_credential_revoker` (`tenant_id`, `revoked_by`),
            CONSTRAINT `fk_service_credential_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_credential_creator`
                FOREIGN KEY (`tenant_id`, `created_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_credential_revoker`
                FOREIGN KEY (`tenant_id`, `revoked_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_credential_status` CHECK (`status` IN ('active', 'revoked'))
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='服务账号 API Key 凭据'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_delegation` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `account_id` BIGINT NOT NULL,
            `user_id` BIGINT NOT NULL,
            `token_mac` BINARY(32) NOT NULL,
            `pepper_version` INT NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'active',
            `version` INT NOT NULL DEFAULT 1,
            `not_before` DATETIME(6) NOT NULL,
            `expires_at` DATETIME(6) NOT NULL,
            `reason` VARCHAR(500) NOT NULL,
            `created_by_user_id` BIGINT NOT NULL,
            `revoked_at` DATETIME(6) DEFAULT NULL,
            `revoked_by` BIGINT DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            `idempotency_key_hash` BINARY(32) NOT NULL,
            `request_fingerprint` BINARY(32) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_delegation_tenant_id` (`tenant_id`, `id`),
            UNIQUE KEY `uq_service_delegation_token_mac` (`token_mac`),
            UNIQUE KEY `uq_service_delegation_idempotency` (`tenant_id`, `user_id`, `idempotency_key_hash`),
            KEY `idx_service_delegation_account` (`tenant_id`, `account_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_delegation_user` (`tenant_id`, `user_id`, `status`, `expires_at`, `id`),
            KEY `idx_service_delegation_expiry` (`status`, `expires_at`, `id`),
            KEY `fk_service_delegation_creator` (`tenant_id`, `created_by_user_id`),
            KEY `fk_service_delegation_revoker` (`tenant_id`, `revoked_by`),
            CONSTRAINT `fk_service_delegation_account`
                FOREIGN KEY (`tenant_id`, `account_id`)
                REFERENCES `sys_service_account` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE,
            CONSTRAINT `fk_service_delegation_user`
                FOREIGN KEY (`tenant_id`, `user_id`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_delegation_creator`
                FOREIGN KEY (`tenant_id`, `created_by_user_id`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `fk_service_delegation_revoker`
                FOREIGN KEY (`tenant_id`, `revoked_by`) REFERENCES `sys_user` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE RESTRICT,
            CONSTRAINT `ck_service_delegation_status` CHECK (`status` IN ('active', 'revoked')),
            CONSTRAINT `ck_service_delegation_window` CHECK (`not_before` < `expires_at`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户显式查询委托'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_delegation_capability` (
            `tenant_id` VARCHAR(64) NOT NULL,
            `delegation_id` BIGINT NOT NULL,
            `capability_key` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            PRIMARY KEY (`tenant_id`, `delegation_id`, `capability_key`),
            KEY `idx_service_delegation_capability` (`tenant_id`, `capability_key`, `delegation_id`),
            CONSTRAINT `fk_service_delegation_capability_delegation`
                FOREIGN KEY (`tenant_id`, `delegation_id`)
                REFERENCES `sys_service_delegation` (`tenant_id`, `id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='委托查询能力白名单'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_service_access_audit` (
            `id` BIGINT NOT NULL,
            `request_id` CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `tenant_id` VARCHAR(64) DEFAULT NULL,
            `account_id` BIGINT DEFAULT NULL,
            `credential_id` BIGINT DEFAULT NULL,
            `delegation_id` BIGINT DEFAULT NULL,
            `represented_user_id` BIGINT DEFAULT NULL,
            `operation_id` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `capability_key` VARCHAR(96) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `required_permission` VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `access_mode` VARCHAR(16) NOT NULL,
            `result` VARCHAR(16) NOT NULL,
            `reason_code` VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
            `http_status` INT NOT NULL,
            `request_ip_digest` BINARY(32) DEFAULT NULL,
            `user_agent_digest` BINARY(32) DEFAULT NULL,
            `row_count` INT DEFAULT NULL,
            `response_bytes` BIGINT DEFAULT NULL,
            `tenant_epoch` INT DEFAULT NULL,
            `account_authorization_version` INT DEFAULT NULL,
            `user_authorization_version` INT DEFAULT NULL,
            `delegation_version` INT DEFAULT NULL,
            `started_at` DATETIME(6) NOT NULL,
            `completed_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_service_access_audit_request` (`request_id`),
            KEY `idx_service_access_audit_retention` (`completed_at`, `id`),
            KEY `idx_service_access_audit_tenant` (`tenant_id`, `completed_at`, `id`),
            KEY `idx_service_access_audit_account` (`tenant_id`, `account_id`, `completed_at`, `id`),
            KEY `idx_service_access_audit_user` (`tenant_id`, `represented_user_id`, `completed_at`, `id`),
            CONSTRAINT `ck_service_access_audit_mode` CHECK (`access_mode` IN ('direct', 'delegated', 'unknown')),
            CONSTRAINT `ck_service_access_audit_result` CHECK (`result` IN ('success', 'denied', 'error')),
            CONSTRAINT `ck_service_access_audit_counts` CHECK (
                (`row_count` IS NULL OR `row_count` >= 0)
                AND (`response_bytes` IS NULL OR `response_bytes` >= 0)
            )
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='Agent API 访问审计'"#,
    ]
}

/// 幂等补齐服务账号管理权限和菜单，不授予任何普通角色。
pub(crate) async fn seed_service_account_management<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    seed_service_account_management_inner(db, false).await
}

async fn seed_service_account_management_inner<C>(
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
        let before = resource_count(db, &tenant_id).await?;
        let parent_id = permission_id(db, &tenant_id, "system")
            .await?
            .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 缺少 system 父权限")))?;
        for (code, name, sort) in SERVICE_ACCOUNT_PERMISSIONS {
            let id = if let Some(id) = permission_id(db, &tenant_id, code).await? {
                validate_permission_definition(db, &tenant_id, id, code, name, parent_id, *sort)
                    .await?;
                id
            } else {
                let id = next_id()?;
                db.execute_raw(Statement::from_sql_and_values(
                    DbBackend::MySql,
                    "INSERT INTO sys_permission (id, tenant_id, name, code, parent_id, perm_type, icon, sort, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
                    [id.into(), tenant_id.clone().into(), (*name).into(), (*code).into(), parent_id.into(), (*sort).into()],
                )).await?;
                id
            };
            if reject_preexisting_role_bindings
                && ordinary_role_has_permission(db, &tenant_id, id).await?
            {
                return Err(DbErr::Custom(format!(
                    "租户 {tenant_id} 的保留服务账号权限 {code} 在迁移前已绑定普通角色"
                )));
            }
        }
        seed_menu(db, &tenant_id).await?;
        if resource_count(db, &tenant_id).await? > before {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE sys_tenant SET configuration_version = configuration_version + 1, authorization_epoch = authorization_epoch + 1, updated_at = UTC_TIMESTAMP(6) WHERE tenant_id = ?",
                [tenant_id.into()],
            )).await?;
        }
    }
    Ok(())
}

async fn seed_menu<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let parent_id = menu_id(db, tenant_id, "system")
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 缺少 system 父菜单")))?;
    let permission_id = permission_id(db, tenant_id, "system:service-account:list")
        .await?
        .ok_or_else(|| DbErr::Custom("服务账号查询权限未创建".into()))?;
    if let Some(id) = menu_id(db, tenant_id, "system.service-accounts").await? {
        validate_menu_definition(db, tenant_id, id, parent_id, permission_id).await?;
        return Ok(());
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_menu (id, tenant_id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, remark, del_flag, created_at, updated_at) VALUES (?, ?, '服务账号', ?, 'C', ?, 'system.service-accounts', 'Key', 14, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [next_id()?.into(), tenant_id.into(), parent_id.into(), permission_id.into()],
    )).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn validate_permission_definition<C>(
    db: &C,
    tenant_id: &str,
    permission_id: i64,
    code: &str,
    expected_name: &str,
    expected_parent_id: i64,
    expected_sort: i32,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT name, parent_id, perm_type, icon, sort, status \
             FROM sys_permission WHERE tenant_id = ? AND id = ? AND code = ? LIMIT 1",
            [tenant_id.into(), permission_id.into(), code.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 的权限 {code} 在校验时消失")))?;
    let name = String::try_get_by_index(&row, 0)?;
    let parent_id = Option::<i64>::try_get_by_index(&row, 1)?;
    let permission_type = String::try_get_by_index(&row, 2)?;
    let icon = Option::<String>::try_get_by_index(&row, 3)?;
    let sort = i32::try_get_by_index(&row, 4)?;
    let status = String::try_get_by_index(&row, 5)?;
    if name != expected_name
        || parent_id != Some(expected_parent_id)
        || permission_type != "api"
        || icon.is_some()
        || sort != expected_sort
        || status != "1"
    {
        return Err(DbErr::Custom(format!(
            "租户 {tenant_id} 的保留权限 {code} 已存在，但定义与服务账号管理契约不一致"
        )));
    }
    Ok(())
}

async fn validate_menu_definition<C>(
    db: &C,
    tenant_id: &str,
    menu_id: i64,
    expected_parent_id: i64,
    expected_permission_id: i64,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT name, parent_id, menu_type, perm_id, icon, sort, \
                    CAST(visible AS SIGNED), status, del_flag \
             FROM sys_menu WHERE tenant_id = ? AND id = ? LIMIT 1",
            [tenant_id.into(), menu_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom(format!("租户 {tenant_id} 的服务账号菜单在校验时消失")))?;
    let name = String::try_get_by_index(&row, 0)?;
    let parent_id = Option::<i64>::try_get_by_index(&row, 1)?;
    let menu_type = String::try_get_by_index(&row, 2)?;
    let permission_id = Option::<i64>::try_get_by_index(&row, 3)?;
    let icon = Option::<String>::try_get_by_index(&row, 4)?;
    let sort = i32::try_get_by_index(&row, 5)?;
    let visible = i64::try_get_by_index(&row, 6)?;
    let status = String::try_get_by_index(&row, 7)?;
    let del_flag = String::try_get_by_index(&row, 8)?;
    if name != "服务账号"
        || parent_id != Some(expected_parent_id)
        || menu_type != "C"
        || permission_id != Some(expected_permission_id)
        || icon.as_deref() != Some("Key")
        || sort != 14
        || visible != 1
        || status != "1"
        || del_flag != "0"
    {
        return Err(DbErr::Custom(format!(
            "租户 {tenant_id} 的保留菜单 system.service-accounts 已存在，但定义与服务账号管理契约不一致"
        )));
    }
    Ok(())
}

async fn resource_count<C>(db: &C, tenant_id: &str) -> Result<i64, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let codes = SERVICE_ACCOUNT_PERMISSIONS
        .iter()
        .map(|(code, _, _)| format!("'{code}'"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT (SELECT COUNT(*) FROM sys_permission WHERE tenant_id = ? AND code IN ({codes})) + (SELECT COUNT(*) FROM sys_menu WHERE tenant_id = ? AND route_key = 'system.service-accounts' AND del_flag = '0')"
    );
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            [tenant_id.into(), tenant_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("服务账号权限资源计数没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)?)
}

async fn ordinary_role_has_permission<C>(
    db: &C,
    tenant_id: &str,
    perm_id: i64,
) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT 1 FROM sys_role_permission relation JOIN sys_role role ON role.tenant_id = relation.tenant_id AND role.id = relation.role_id WHERE relation.tenant_id = ? AND relation.perm_id = ? AND role.is_super = 0 AND role.del_flag = '0' LIMIT 1",
        [tenant_id.into(), perm_id.into()],
    )).await?.is_some())
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
    Ok(db.query_one_raw(Statement::from_sql_and_values(
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
    let row = db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
        [table.into()],
    )).await?.ok_or_else(|| DbErr::Custom("表存在性查询没有返回记录".into()))?;
    Ok(i64::try_get_by_index(&row, 0)? > 0)
}

fn next_id() -> Result<i64, DbErr> {
    ryframe_utils::snowflake::try_next_snowflake_id()
        .map_err(|error| DbErr::Custom(error.to_string()))
}

/// 将历史基线映射为包含租户复合身份键的当前审阅快照。
pub(crate) fn current_snapshot_statement(statement: &str) -> Cow<'_, str> {
    for (table, index) in [
        ("sys_dept", "uq_sys_dept_tenant_id"),
        ("sys_user", "uq_sys_user_tenant_id"),
        ("sys_role", "uq_sys_role_tenant_id"),
    ] {
        if statement
            .trim_start()
            .starts_with(&format!("CREATE TABLE IF NOT EXISTS `{table}`"))
        {
            let current = statement.replacen(
                "    PRIMARY KEY (`id`),",
                &format!("    PRIMARY KEY (`id`),\n    UNIQUE KEY `{index}` (`tenant_id`, `id`),"),
                1,
            );
            assert_ne!(
                current, statement,
                "服务账号审阅快照无法在 {table} 中安装租户复合身份键"
            );
            return Cow::Owned(current);
        }
    }
    Cow::Borrowed(statement)
}
