use sea_orm::{ConnectionTrait, DbBackend};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_menu").await? {
            return Ok(());
        }

        if !manager.has_column("sys_menu", "perm_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_menu"))
                        .add_column(ColumnDef::new(Alias::new("perm_id")).big_integer().null())
                        .to_owned(),
                )
                .await?;
        }
        if !manager.has_column("sys_menu", "route_key").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("sys_menu"))
                        .add_column(
                            ColumnDef::new(Alias::new("route_key"))
                                .string_len(100)
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_index(
                Index::create()
                    .name("idx_menu_tenant_perm")
                    .table(Alias::new("sys_menu"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("perm_id"))
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_menu_tenant_route")
                    .table(Alias::new("sys_menu"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("route_key"))
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        backfill_route_keys(manager).await?;
        backfill_permission_ids(manager).await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Existing menu bindings are operational data. Do not discard them.
        Ok(())
    }
}

async fn backfill_route_keys(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .get_connection()
        .execute_unprepared(
            "UPDATE sys_menu SET route_key = CASE name \
             WHEN '首页' THEN 'home' WHEN '系统管理' THEN 'system' \
             WHEN '系统监控' THEN 'monitor' WHEN '系统工具' THEN 'tools' \
             WHEN '用户管理' THEN 'system.user' WHEN '角色管理' THEN 'system.role' \
             WHEN '菜单管理' THEN 'system.menu' WHEN '部门管理' THEN 'system.dept' \
             WHEN '岗位管理' THEN 'system.post' WHEN '字典管理' THEN 'system.dict' \
             WHEN '参数设置' THEN 'system.config' WHEN '通知公告' THEN 'system.notice' \
             WHEN '权限管理' THEN 'system.perm' WHEN '操作日志' THEN 'system.operlog' \
             WHEN '登录日志' THEN 'system.logininfor' WHEN '在线用户' THEN 'monitor.online' \
             WHEN '服务监控' THEN 'monitor.server' WHEN '运行时监控' THEN 'monitor.runtime' \
             WHEN '缓存监控' THEN 'monitor.cache' WHEN '连接池监控' THEN 'monitor.db-pool' \
             WHEN '代码生成' THEN 'tools.gen' ELSE route_key END \
             WHERE route_key IS NULL AND menu_type IN ('M', 'C')",
        )
        .await?;
    Ok(())
}

async fn backfill_permission_ids(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    if !manager.has_table("sys_permission").await? {
        return Ok(());
    }

    let connection = manager.get_connection();
    let expression = "CASE m.name \
        WHEN '用户管理' THEN 'system:user:list' WHEN '角色管理' THEN 'system:role:list' \
        WHEN '菜单管理' THEN 'system:menu:list' WHEN '部门管理' THEN 'system:dept:list' \
        WHEN '岗位管理' THEN 'system:post:list' WHEN '字典管理' THEN 'system:dict:list' \
        WHEN '参数设置' THEN 'system:config:list' WHEN '通知公告' THEN 'system:notice:list' \
        WHEN '权限管理' THEN 'system:perm:list' WHEN '操作日志' THEN 'system:operlog:list' \
        WHEN '登录日志' THEN 'system:logininfor:list' WHEN '在线用户' THEN 'monitor:online:list' \
        WHEN '服务监控' THEN 'monitor:server:list' WHEN '运行时监控' THEN 'monitor:runtime:list' \
        WHEN '缓存监控' THEN 'monitor:cache:list' WHEN '连接池监控' THEN 'monitor:db-pool:list' \
        WHEN '代码生成' THEN 'tools:gen:list' WHEN '用户查询' THEN 'system:user:list' \
        WHEN '用户新增' THEN 'system:user:add' WHEN '用户修改' THEN 'system:user:edit' \
        WHEN '用户删除' THEN 'system:user:remove' WHEN '用户导出' THEN 'system:user:export' \
        ELSE NULL END";

    let sql = match connection.get_database_backend() {
        DbBackend::MySql => format!(
            "UPDATE sys_menu m LEFT JOIN sys_permission p \
             ON p.tenant_id = m.tenant_id AND p.code = {expression} \
             SET m.perm_id = p.id WHERE m.perm_id IS NULL"
        ),
        _ => format!(
            "UPDATE sys_menu AS m SET perm_id = (SELECT p.id FROM sys_permission p \
             WHERE p.tenant_id = m.tenant_id AND p.code = {expression} LIMIT 1) \
             WHERE m.perm_id IS NULL"
        ),
    };
    connection.execute_unprepared(&sql).await?;
    Ok(())
}
