use sea_orm_migration::prelude::*;

const PLATFORM_MESSAGE_PUBLISH_PERMISSION: &str = "platform:message:publish";
const SYSTEM_TENANT_ID: &str = "system";

/// 收紧早期消息权限迁移错误授予普通租户的平台能力。
///
/// 该迁移仅删除非 system 租户中的平台级权限及其角色关联，不影响同名业务权限或
/// system 租户的平台管理员。菜单保留但解除权限关联，避免因历史自定义菜单而删除
/// 用户的页面配置。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("sys_permission").await? {
            return Ok(());
        }

        if manager.has_table("sys_menu").await? {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "UPDATE sys_menu AS m \
                     INNER JOIN sys_permission AS p ON p.id = m.perm_id \
                     SET m.perm_id = NULL \
                     WHERE p.tenant_id <> '{SYSTEM_TENANT_ID}' \
                       AND p.code = '{PLATFORM_MESSAGE_PUBLISH_PERMISSION}'"
                ))
                .await?;
        }

        if manager.has_table("sys_role_permission").await? {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "DELETE rp FROM sys_role_permission AS rp \
                     INNER JOIN sys_permission AS p ON p.id = rp.perm_id \
                     WHERE p.tenant_id <> '{SYSTEM_TENANT_ID}' \
                       AND p.code = '{PLATFORM_MESSAGE_PUBLISH_PERMISSION}'"
                ))
                .await?;
        }

        manager
            .get_connection()
            .execute_unprepared(&format!(
                "DELETE FROM sys_permission \
                 WHERE tenant_id <> '{SYSTEM_TENANT_ID}' \
                   AND code = '{PLATFORM_MESSAGE_PUBLISH_PERMISSION}'"
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "平台权限作用域修复为前向兼容数据，不能自动恢复已撤销的授权".into(),
        ))
    }
}
