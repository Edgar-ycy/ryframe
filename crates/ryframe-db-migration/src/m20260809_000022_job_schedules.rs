use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

/// 增加租户隔离的 Cron 调度、执行历史和管理端权限菜单。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("job schedules require MySQL 8.0+".into()));
        }

        add_background_job_columns(manager).await?;
        create_schedule_tables(manager).await?;
        seed_schedule_management(manager.get_connection()).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "调度计划与执行历史为前向兼容数据，不能自动删除".into(),
        ))
    }
}

async fn add_background_job_columns(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for (name, definition) in [
        ("schedule_id", "BIGINT NULL AFTER `tenant_id`"),
        ("scheduled_for", "DATETIME(6) NULL AFTER `schedule_id`"),
        ("max_runtime_seconds", "INT NULL AFTER `scheduled_for`"),
    ] {
        if !manager.has_column("sys_background_job", name).await? {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE `sys_background_job` ADD COLUMN `{name}` {definition}"
                ))
                .await?;
        }
    }
    if !manager
        .has_index("sys_background_job", "idx_bg_job_schedule_status")
        .await?
    {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX `idx_bg_job_schedule_status` ON `sys_background_job` \
                 (`schedule_id`, `status`, `created_at`)",
            )
            .await?;
    }
    Ok(())
}

async fn create_schedule_tables(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .get_connection()
        .execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS `sys_job_schedule` (
                `id` BIGINT NOT NULL,
                `tenant_id` VARCHAR(64) NOT NULL,
                `name` VARCHAR(100) NOT NULL,
                `handler_key` VARCHAR(96) NOT NULL,
                `cron_expression` VARCHAR(191) NOT NULL,
                `timezone` VARCHAR(64) NOT NULL,
                `enabled` TINYINT(1) NOT NULL DEFAULT 1,
                `misfire_policy` VARCHAR(16) NOT NULL DEFAULT 'fire_once',
                `concurrency_policy` VARCHAR(16) NOT NULL DEFAULT 'forbid',
                `max_runtime_seconds` INT NOT NULL DEFAULT 900,
                `next_run_at` DATETIME(6) DEFAULT NULL,
                `last_run_at` DATETIME(6) DEFAULT NULL,
                `version` BIGINT NOT NULL DEFAULT 1,
                `del_flag` CHAR(1) NOT NULL DEFAULT '0',
                `created_at` DATETIME(6) NOT NULL,
                `updated_at` DATETIME(6) NOT NULL,
                PRIMARY KEY (`id`),
                KEY `idx_job_schedule_scan` (`enabled`, `del_flag`, `next_run_at`, `id`),
                KEY `idx_job_schedule_tenant` (`tenant_id`, `del_flag`, `created_at`)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
        )
        .await?;
    manager
        .get_connection()
        .execute_unprepared(
            r#"CREATE TABLE IF NOT EXISTS `sys_job_schedule_execution` (
                `id` BIGINT NOT NULL,
                `tenant_id` VARCHAR(64) NOT NULL,
                `schedule_id` BIGINT NOT NULL,
                `schedule_name_snapshot` VARCHAR(100) NOT NULL,
                `handler_key_snapshot` VARCHAR(96) NOT NULL,
                `fire_key` VARCHAR(191) NOT NULL,
                `trigger_kind` VARCHAR(16) NOT NULL,
                `scheduled_for` DATETIME(6) NOT NULL,
                `outcome` VARCHAR(32) NOT NULL,
                `background_job_id` BIGINT DEFAULT NULL,
                `detail` TEXT DEFAULT NULL,
                `created_at` DATETIME(6) NOT NULL,
                PRIMARY KEY (`id`),
                UNIQUE KEY `uq_job_schedule_fire` (`schedule_id`, `fire_key`),
                KEY `idx_job_schedule_execution_history`
                    (`tenant_id`, `schedule_id`, `created_at`)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci"#,
        )
        .await?;
    Ok(())
}

/// 在规范种子写入后或升级已有数据库时幂等补齐调度权限、菜单和平台计划。
pub(crate) async fn seed_schedule_management<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_permission").await? {
        return Ok(());
    }
    let tenants = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT DISTINCT tenant_id FROM sys_permission".to_owned(),
        ))
        .await?;
    for row in tenants {
        let tenant_id = String::try_get_by_index(&row, 0)?;
        seed_tenant_permissions(db, &tenant_id).await?;
        seed_tenant_menus(db, &tenant_id).await?;
    }
    seed_system_schedules(db).await
}

async fn seed_tenant_permissions<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let monitor_id = permission_id(db, tenant_id, "monitor").await?;
    for (code, name, sort) in [
        ("monitor:job:list", "任务监控查询", 10),
        ("monitor:job:retry", "任务人工重试", 11),
        ("monitor:schedule:list", "定时任务查询", 20),
        ("monitor:schedule:add", "定时任务新增", 21),
        ("monitor:schedule:edit", "定时任务修改", 22),
        ("monitor:schedule:remove", "定时任务删除", 23),
        ("monitor:schedule:run", "定时任务立即执行", 24),
    ] {
        if let Some(id) = permission_id(db, tenant_id, code).await? {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "UPDATE sys_permission SET parent_id = ?, name = ?, sort = ?, updated_at = UTC_TIMESTAMP(6) WHERE id = ? AND tenant_id = ?",
                [
                    monitor_id.into(),
                    name.into(),
                    sort.into(),
                    id.into(),
                    tenant_id.into(),
                ],
            ))
            .await?;
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
                tenant_id.into(),
                name.into(),
                code.into(),
                monitor_id.into(),
                sort.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn seed_tenant_menus<C>(db: &C, tenant_id: &str) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let Some(parent_id) = menu_id_by_route(db, tenant_id, "monitor").await? else {
        return Ok(());
    };
    for (name, route_key, permission_code, icon, sort) in [
        ("后台任务", "monitor.jobs", "monitor:job:list", "List", 6),
        (
            "定时任务",
            "monitor.schedules",
            "monitor:schedule:list",
            "Timer",
            7,
        ),
    ] {
        if menu_id_by_route(db, tenant_id, route_key).await?.is_some() {
            continue;
        }
        let Some(perm_id) = permission_id(db, tenant_id, permission_code).await? else {
            continue;
        };
        let id = ryframe_utils::snowflake::try_next_snowflake_id()
            .map_err(|error| DbErr::Custom(error.to_string()))?;
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_menu \
             (id, tenant_id, name, parent_id, menu_type, perm_id, route_key, icon, sort, visible, status, remark, del_flag, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'C', ?, ?, ?, ?, 1, '1', NULL, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [
                id.into(),
                tenant_id.into(),
                name.into(),
                parent_id.into(),
                perm_id.into(),
                route_key.into(),
                icon.into(),
                sort.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn seed_system_schedules<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_job_schedule").await? {
        return Ok(());
    }
    let system_exists = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id FROM sys_tenant WHERE tenant_id = ? LIMIT 1",
            ["system".into()],
        ))
        .await?
        .is_some();
    if !system_exists {
        return Ok(());
    }
    for (id, name, handler_key) in [
        (1_i64, "导出结果过期清理", "system.export_result_cleanup"),
        (2_i64, "消息过期清理", "system.message_retention_cleanup"),
    ] {
        let exists = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                "SELECT id FROM sys_job_schedule WHERE tenant_id = 'system' AND handler_key = ? LIMIT 1",
                [handler_key.into()],
            ))
            .await?
            .is_some();
        if exists {
            continue;
        }
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "INSERT INTO sys_job_schedule \
             (id, tenant_id, name, handler_key, cron_expression, timezone, enabled, misfire_policy, concurrency_policy, max_runtime_seconds, next_run_at, last_run_at, version, del_flag, created_at, updated_at) \
             VALUES (?, 'system', ?, ?, '0 5 0 * * * *', 'UTC', 1, 'fire_once', 'forbid', 900, UTC_TIMESTAMP(6), NULL, 1, '0', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
            [id.into(), name.into(), handler_key.into()],
        ))
        .await?;
    }
    Ok(())
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

async fn menu_id_by_route<C>(db: &C, tenant_id: &str, route_key: &str) -> Result<Option<i64>, DbErr>
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

async fn table_exists<C>(db: &C, table: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT COUNT(*) AS count FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            [table.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("table existence query returned no row".into()))?;
    Ok(i64::try_get_by_index(&row, 0)? > 0)
}
