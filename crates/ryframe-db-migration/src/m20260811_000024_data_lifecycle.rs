use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

/// 增加数据保留运行记录、异步用户导入及清理和趋势索引。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "data lifecycle requires MySQL 8.0.16 or newer".into(),
            ));
        }
        create_lifecycle_tables(manager).await?;
        add_lifecycle_indexes(manager).await?;
        seed_lifecycle_management(manager.get_connection()).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "数据保留和导入历史属于前向业务数据，不能自动删除".into(),
        ))
    }
}

async fn create_lifecycle_tables(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for statement in lifecycle_table_statements() {
        manager
            .get_connection()
            .execute_unprepared(statement)
            .await?;
    }
    Ok(())
}

pub(crate) fn lifecycle_table_statements() -> [&'static str; 3] {
    [
        r#"CREATE TABLE IF NOT EXISTS `sys_data_retention_run` (
            `id` BIGINT NOT NULL,
            `background_job_id` BIGINT NOT NULL,
            `trigger_kind` VARCHAR(16) NOT NULL,
            `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `policy_snapshot` JSON NOT NULL,
            `eligible_counts` JSON NOT NULL,
            `deleted_counts` JSON NOT NULL,
            `remaining_counts` JSON NOT NULL,
            `requested_by` BIGINT DEFAULT NULL,
            `error_summary` TEXT DEFAULT NULL,
            `started_at` DATETIME(6) DEFAULT NULL,
            `completed_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_retention_run_background_job` (`background_job_id`),
            KEY `idx_retention_run_created` (`created_at`, `id`),
            KEY `idx_retention_run_history` (`status`, `completed_at`, `id`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='数据保留运行记录'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_user_import_job` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `requester_user_id` BIGINT NOT NULL,
            `background_job_id` BIGINT NOT NULL,
            `idempotency_key_hash` CHAR(64) NOT NULL,
            `source_file_id` BIGINT NOT NULL,
            `source_name_snapshot` VARCHAR(255) NOT NULL,
            `source_sha256` CHAR(64) NOT NULL,
            `duplicate_policy` VARCHAR(24) NOT NULL DEFAULT 'skip_existing',
            `status` VARCHAR(16) NOT NULL DEFAULT 'pending',
            `total_rows` INT NOT NULL DEFAULT 0,
            `processed_rows` INT NOT NULL DEFAULT 0,
            `success_count` INT NOT NULL DEFAULT 0,
            `skipped_count` INT NOT NULL DEFAULT 0,
            `failure_count` INT NOT NULL DEFAULT 0,
            `cancel_requested` TINYINT(1) NOT NULL DEFAULT 0,
            `error_report_file_id` BIGINT DEFAULT NULL,
            `last_error` TEXT DEFAULT NULL,
            `started_at` DATETIME(6) DEFAULT NULL,
            `completed_at` DATETIME(6) DEFAULT NULL,
            `created_at` DATETIME(6) NOT NULL,
            `updated_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_user_import_idempotency` (`tenant_id`, `idempotency_key_hash`),
            UNIQUE KEY `uq_user_import_background_job` (`background_job_id`),
            KEY `idx_user_import_tenant_status` (`tenant_id`, `status`, `created_at`),
            KEY `idx_user_import_history` (`completed_at`, `id`)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='异步用户导入任务'"#,
        r#"CREATE TABLE IF NOT EXISTS `sys_user_import_row_result` (
            `id` BIGINT NOT NULL,
            `tenant_id` VARCHAR(64) NOT NULL,
            `import_job_id` BIGINT NOT NULL,
            `row_number` INT NOT NULL,
            `username_snapshot` VARCHAR(64) NOT NULL,
            `outcome` VARCHAR(16) NOT NULL,
            `code` VARCHAR(64) NOT NULL,
            `message` VARCHAR(500) NOT NULL,
            `created_at` DATETIME(6) NOT NULL,
            PRIMARY KEY (`id`),
            UNIQUE KEY `uq_user_import_row` (`import_job_id`, `row_number`),
            KEY `idx_user_import_row_tenant` (`tenant_id`, `import_job_id`, `row_number`),
            CONSTRAINT `fk_user_import_row_job`
                FOREIGN KEY (`import_job_id`) REFERENCES `sys_user_import_job` (`id`)
                ON UPDATE CASCADE ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci COMMENT='用户导入异常行结果'"#,
    ]
}

async fn add_lifecycle_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for (table, name, columns) in [
        (
            "sys_background_job",
            "idx_bg_job_retention",
            "`status`, `completed_at`, `id`",
        ),
        (
            "sys_background_job",
            "idx_bg_job_tenant_created_status",
            "`tenant_id`, `created_at`, `status`",
        ),
        (
            "sys_outbox_event",
            "idx_outbox_event_retention",
            "`status`, `published_at`, `id`",
        ),
        (
            "sys_job_schedule_execution",
            "idx_job_schedule_execution_retention",
            "`created_at`, `id`",
        ),
        (
            "sys_job_schedule_execution",
            "idx_job_schedule_execution_trend",
            "`tenant_id`, `created_at`, `outcome`",
        ),
        (
            "sys_export_job",
            "idx_export_job_history",
            "`status`, `completed_at`, `id`",
        ),
        (
            "sys_oper_log",
            "idx_oper_log_tenant_time_status",
            "`tenant_id`, `oper_time`, `status`",
        ),
        (
            "sys_login_info",
            "idx_login_info_tenant_time_status",
            "`tenant_id`, `login_time`, `status`",
        ),
    ] {
        if !manager.has_index(table, name).await? {
            manager
                .get_connection()
                .execute_unprepared(&format!("CREATE INDEX `{name}` ON `{table}` ({columns})"))
                .await?;
        }
    }
    Ok(())
}

/// 幂等补齐本轮权限、菜单和系统默认保留计划。
pub(crate) async fn seed_lifecycle_management<C>(db: &C) -> Result<(), DbErr>
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
    let mut has_system_tenant = false;
    for row in tenants {
        let tenant_id = String::try_get_by_index(&row, 0)?;
        has_system_tenant |= tenant_id == "system";
        seed_tenant_permissions(db, &tenant_id).await?;
        seed_tenant_menus(db, &tenant_id).await?;
    }
    // 全新数据库执行增量迁移时，规范租户会在迁移完成后的 Seeder 阶段创建。
    // 此时只安装表和索引，避免在父租户尚不存在时触发外键约束。
    if !has_system_tenant {
        return Ok(());
    }
    seed_system_retention_permissions(db).await?;
    seed_system_retention_menu(db).await?;
    seed_system_retention_schedule(db).await
}

async fn seed_tenant_permissions<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for (parent_code, code, name, sort) in [
        ("system:user", "system:user-import:list", "用户导入查询", 30),
        ("system:user", "system:user-import:add", "用户导入新增", 31),
        (
            "system:user",
            "system:user-import:cancel",
            "用户导入取消",
            32,
        ),
        (
            "system",
            "system:authorization-diagnostic:list",
            "权限生效诊断",
            40,
        ),
        ("monitor", "monitor:overview:list", "运维总览查询", 5),
    ] {
        seed_permission(db, tenant_id, parent_code, code, name, sort).await?;
    }
    Ok(())
}

async fn seed_system_retention_permissions<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for (code, name, sort) in [
        ("monitor:retention:list", "数据保留查询", 30),
        ("monitor:retention:run", "数据保留运行", 31),
    ] {
        seed_permission(db, "system", "monitor", code, name, sort).await?;
    }
    Ok(())
}

async fn seed_permission<C>(
    db: &C,
    tenant_id: &str,
    parent_code: &str,
    code: &str,
    name: &str,
    sort: i32,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if permission_id(db, tenant_id, code).await?.is_some() {
        return Ok(());
    }
    let parent_id = permission_id(db, tenant_id, parent_code).await?;
    let id = next_id()?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_permission \
         (id, tenant_id, name, code, parent_id, perm_type, icon, sort, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'api', NULL, ?, '1', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            id.into(),
            tenant_id.into(),
            name.into(),
            code.into(),
            parent_id.into(),
            sort.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn seed_tenant_menus<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    for (parent_route, name, route_key, permission_code, icon, sort) in [
        (
            "monitor",
            "运维总览",
            "monitor.overview",
            "monitor:overview:list",
            "TrendCharts",
            1,
        ),
        (
            "system",
            "权限诊断",
            "system.authorization-diagnostics",
            "system:authorization-diagnostic:list",
            "View",
            12,
        ),
    ] {
        seed_menu(
            db,
            tenant_id,
            MenuSeed {
                parent_route,
                name,
                route_key,
                permission_code,
                icon,
                sort,
            },
        )
        .await?;
    }
    Ok(())
}

async fn seed_system_retention_menu<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    seed_menu(
        db,
        "system",
        MenuSeed {
            parent_route: "monitor",
            name: "数据保留",
            route_key: "monitor.retention",
            permission_code: "monitor:retention:list",
            icon: "DeleteFilled",
            sort: 8,
        },
    )
    .await
}

struct MenuSeed<'a> {
    parent_route: &'a str,
    name: &'a str,
    route_key: &'a str,
    permission_code: &'a str,
    icon: &'a str,
    sort: i32,
}

async fn seed_menu<C>(db: &C, tenant_id: &str, menu: MenuSeed<'_>) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if menu_id(db, tenant_id, menu.route_key).await?.is_some() {
        return Ok(());
    }
    let Some(parent_id) = menu_id(db, tenant_id, menu.parent_route).await? else {
        return Ok(());
    };
    let Some(perm_id) = permission_id(db, tenant_id, menu.permission_code).await? else {
        return Ok(());
    };
    let id = next_id()?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_menu \
         (id, tenant_id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, remark, del_flag, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 'C', ?, ?, ?, ?, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [
            id.into(),
            tenant_id.into(),
            menu.name.into(),
            parent_id.into(),
            perm_id.into(),
            menu.route_key.into(),
            menu.icon.into(),
            menu.sort.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn seed_system_retention_schedule<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_job_schedule").await?
        || schedule_exists(db, "system.data_retention_cleanup").await?
    {
        return Ok(());
    }
    let id = next_id()?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "INSERT INTO sys_job_schedule \
         (id, tenant_id, name, handler_key, cron_expression, timezone, enabled, misfire_policy, concurrency_policy, max_runtime_seconds, next_run_at, last_run_at, version, del_flag, created_at, updated_at) \
         VALUES (?, 'system', '数据保留清理', 'system.data_retention_cleanup', '0 30 3 * * * *', 'UTC', 1, 'fire_once', 'forbid', 900, \
         CASE WHEN UTC_TIME(6) < '03:30:00' THEN TIMESTAMP(UTC_DATE(), '03:30:00') ELSE TIMESTAMP(DATE_ADD(UTC_DATE(), INTERVAL 1 DAY), '03:30:00') END, \
         NULL, 1, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        [id.into()],
    ))
    .await?;
    Ok(())
}

fn next_id() -> Result<i64, DbErr> {
    ryframe_utils::snowflake::try_next_snowflake_id()
        .map_err(|error| DbErr::Custom(error.to_string()))
}

async fn permission_id<C>(db: &C, tenant_id: &str, code: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_permission WHERE tenant_id = ? AND code = ? LIMIT 1",
            [tenant_id.into(), code.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn menu_id<C>(db: &C, tenant_id: &str, route_key: &str) -> Result<Option<i64>, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_menu WHERE tenant_id = ? AND route_key = ? AND del_flag = '0' LIMIT 1",
            [tenant_id.into(), route_key.into()],
        ))
        .await?;
    Ok(row.map(|row| i64::try_get_by_index(&row, 0)).transpose()?)
}

async fn schedule_exists<C>(db: &C, handler_key: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_job_schedule WHERE tenant_id = 'system' AND handler_key = ? LIMIT 1",
            [handler_key.into()],
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
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            [table.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("table existence query returned no row".into()))?;
    Ok(i64::try_get_by_index(&row, 0)? > 0)
}
