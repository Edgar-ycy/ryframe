use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

/// 插入权限管理菜单和权限数据（补充 ryframe_config.sql 中的新增条目）
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260613_000003_seed_permission_menu"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // 插入 sys_permission 权限管理菜单条目（幂等：IGNORE 避免重复插入报错）
        conn.execute_unprepared(
            "INSERT IGNORE INTO `sys_permission` (`id`, `name`, `code`, `parent_id`, `perm_type`, `path`, `http_method`, `icon`, `sort`, `status`) VALUES \
             (69, '权限管理', 'system:permission', 1, 'menu', '/system/permission', NULL, 'Key', 12, '1'), \
             (70, '权限查询', 'system:permission:list', 69, 'api', 'system/permissions/tree', 'GET', NULL, 1, '1')",
        )
        .await?;

        // 插入 sys_menu 权限管理菜单
        conn.execute_unprepared(
            "INSERT IGNORE INTO `sys_menu` (`id`, `name`, `parent_id`, `menu_type`, `path`, `component`, `query`, `perms`, `icon`, `is_frame`, `is_cache`, `sort`, `visible`, `status`) VALUES \
             (23, '权限管理', 1, 'C', '/system/permission', 'system/permission/index', NULL, 'system:permission:list', 'Key', 0, 0, 12, 1, '1')",
        )
        .await?;

        // 为超级管理员(role_id=1)分配权限管理菜单
        conn.execute_unprepared(
            "INSERT IGNORE INTO `role_menu` (`role_id`, `menu_id`) VALUES (1, 23)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DELETE FROM `role_menu` WHERE `role_id` = 1 AND `menu_id` = 23")
            .await?;
        conn.execute_unprepared("DELETE FROM `sys_menu` WHERE `id` = 23")
            .await?;
        conn.execute_unprepared("DELETE FROM `sys_permission` WHERE `id` IN (69, 70)")
            .await?;
        Ok(())
    }
}
