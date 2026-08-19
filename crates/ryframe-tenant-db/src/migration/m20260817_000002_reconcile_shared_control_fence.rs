use sea_orm::DbBackend;
use sea_orm_migration::prelude::*;

/// 修复早期 shared-control 双账分别生成随机 switch token 导致的围栏不一致。
///
/// 只触碰首次安装的 shared-control/generation=1/active 记录；维护、迁移及后续代际
/// 均保持原样，因此重跑不会越过正在进行的放置切换。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom(
                "tenant-data fence reconciliation requires MySQL".into(),
            ));
        }
        if !manager.has_table("sys_tenant_data_placement").await? {
            return Ok(());
        }

        manager
            .get_connection()
            .execute_unprepared(
                r#"UPDATE `biz_tenant_fence` AS fence
                   INNER JOIN `sys_tenant_data_placement` AS placement
                     ON placement.`tenant_id` = fence.`tenant_id`
                   SET fence.`switch_token` = placement.`switch_token`,
                       fence.`updated_at` = CURRENT_TIMESTAMP(6)
                   WHERE placement.`current_target_key` = 'shared-control'
                     AND placement.`placement_generation` = 1
                     AND placement.`state` = 'active'
                     AND fence.`target_key` = 'shared-control'
                     AND fence.`placement_generation` = 1
                     AND fence.`state` = 'active'
                     AND fence.`switch_token` <> placement.`switch_token`"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // 数据修复不可逆；回滚账本不应重新制造不一致 token。
        Ok(())
    }
}
