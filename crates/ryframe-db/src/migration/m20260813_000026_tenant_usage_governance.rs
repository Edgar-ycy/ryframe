use std::borrow::Cow;

use sea_orm::{ConnectionTrait, DbBackend, Statement, TryGetable};
use sea_orm_migration::prelude::*;

const SYSTEM_TENANT_ID: &str = "system";
const USAGE_PERMISSION_CODE: &str = "tenant:usage:list";
const USAGE_PERMISSION_NAME: &str = "租户用量查询";
const USAGE_PERMISSION_SORT: i32 = 5;

const USAGE_INDEXES: &[(&str, &str, &str)] = &[
    ("sys_user", "idx_user_tenant_del", "`tenant_id`, `del_flag`"),
    (
        "sys_role",
        "idx_role_tenant_del",
        "`tenant_id`, `del_flag`, `id`",
    ),
    (
        "sys_file",
        "idx_file_tenant_del_size",
        "`tenant_id`, `del_flag`, `file_size`",
    ),
    (
        "sys_job_schedule",
        "idx_schedule_tenant_del_enabled",
        "`tenant_id`, `enabled`, `del_flag`",
    ),
];

/// 安装租户容量聚合索引，并补齐仅属于系统租户的用量查看权限。
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::MySql {
            return Err(DbErr::Custom("租户容量治理迁移仅支持 MySQL".into()));
        }
        add_usage_indexes(manager).await?;
        seed_tenant_usage_governance_with_policy(manager.get_connection(), true).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Err(DbErr::Custom(
            "租户容量治理权限和索引属于前向兼容数据，不能自动删除".into(),
        ))
    }
}

async fn add_usage_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for (table, name, columns) in USAGE_INDEXES {
        if !manager.has_table(*table).await? {
            return Err(DbErr::Custom(format!(
                "缺少 {table}，无法安装租户容量聚合索引"
            )));
        }
        if !manager.has_index(*table, *name).await? {
            manager
                .get_connection()
                .execute_unprepared(&format!("CREATE INDEX `{name}` ON `{table}` ({columns})"))
                .await?;
        }
    }
    Ok(())
}

/// 幂等补齐系统租户的用量权限，但绝不授予普通角色。
///
/// `tenant:usage:list` 是平台保留代码。任何普通租户同名权限、系统租户中的定义碰撞，
/// 或迁移前已经存在的普通角色绑定都会使迁移失败；迁移完成后仍可按职责显式授权。
pub(crate) async fn seed_tenant_usage_governance<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    seed_tenant_usage_governance_with_policy(db, false).await
}

async fn seed_tenant_usage_governance_with_policy<C>(
    db: &C,
    reject_preexisting_binding: bool,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if !table_exists(db, "sys_permission").await? || !tenant_exists(db, SYSTEM_TENANT_ID).await? {
        // 全新数据库在全部迁移结束后才写入规范种子，此处由 Seeder 再次调用。
        return Ok(());
    }

    reject_non_system_collision(db).await?;
    let parent_id = permission_id(db, SYSTEM_TENANT_ID, "tenant:manage")
        .await?
        .ok_or_else(|| DbErr::Custom("系统租户缺少 tenant:manage 父权限".into()))?;

    if let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT id, name, parent_id, perm_type, icon, sort, status \
             FROM sys_permission WHERE tenant_id = ? AND code = ? LIMIT 1",
            [SYSTEM_TENANT_ID.into(), USAGE_PERMISSION_CODE.into()],
        ))
        .await?
    {
        let permission_id = i64::try_get_by_index(&row, 0)?;
        let name = String::try_get_by_index(&row, 1)?;
        let actual_parent_id = Option::<i64>::try_get_by_index(&row, 2)?;
        let permission_type = String::try_get_by_index(&row, 3)?;
        let icon = Option::<String>::try_get_by_index(&row, 4)?;
        let sort = i32::try_get_by_index(&row, 5)?;
        let status = String::try_get_by_index(&row, 6)?;
        if name != USAGE_PERMISSION_NAME
            || actual_parent_id != Some(parent_id)
            || permission_type != "api"
            || icon.is_some()
            || sort != USAGE_PERMISSION_SORT
            || status != "1"
        {
            return Err(DbErr::Custom(format!(
                "系统租户的保留权限代码 {USAGE_PERMISSION_CODE} 已存在，但定义与租户容量治理权限不一致"
            )));
        }
        if reject_preexisting_binding {
            reject_preexisting_ordinary_role_binding(db, permission_id).await?;
        }
        return Ok(());
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
            USAGE_PERMISSION_NAME.into(),
            USAGE_PERMISSION_CODE.into(),
            parent_id.into(),
            USAGE_PERMISSION_SORT.into(),
        ],
    ))
    .await?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::MySql,
        "UPDATE sys_tenant SET configuration_version = configuration_version + 1, \
         authorization_epoch = authorization_epoch + 1, updated_at = UTC_TIMESTAMP(6) \
         WHERE tenant_id = ?",
        [SYSTEM_TENANT_ID.into()],
    ))
    .await?;
    Ok(())
}

async fn reject_preexisting_ordinary_role_binding<C>(
    db: &C,
    permission_id: i64,
) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT 1 FROM sys_role_permission relation \
             JOIN sys_role role ON role.tenant_id = relation.tenant_id AND role.id = relation.role_id \
             WHERE relation.tenant_id = ? AND relation.perm_id = ? AND role.is_super = 0 LIMIT 1",
            [SYSTEM_TENANT_ID.into(), permission_id.into()],
        ))
        .await?
        .is_some()
    {
        return Err(DbErr::Custom(format!(
            "系统租户的保留权限代码 {USAGE_PERMISSION_CODE} 在迁移前已绑定普通角色，拒绝自动接管"
        )));
    }
    Ok(())
}

async fn reject_non_system_collision<C>(db: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait + ?Sized,
{
    if db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT tenant_id FROM sys_permission \
             WHERE tenant_id <> ? AND code = ? ORDER BY tenant_id LIMIT 1",
            [SYSTEM_TENANT_ID.into(), USAGE_PERMISSION_CODE.into()],
        ))
        .await?
        .is_some()
    {
        return Err(DbErr::Custom(format!(
            "普通租户存在平台保留权限代码 {USAGE_PERMISSION_CODE}，请先完成权限冲突治理"
        )));
    }
    Ok(())
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

/// 将历史基线表声明映射为容量治理完成后的当前审阅快照。
pub(crate) fn current_snapshot_statement(statement: &str) -> Cow<'_, str> {
    let replacements = [
        (
            "sys_user",
            "    KEY `idx_tenant_id` (`tenant_id`),",
            "    KEY `idx_tenant_id` (`tenant_id`),\n    KEY `idx_user_tenant_del` (`tenant_id`, `del_flag`),",
        ),
        (
            "sys_role",
            "    KEY `idx_tenant_id` (`tenant_id`),",
            "    KEY `idx_tenant_id` (`tenant_id`),\n    KEY `idx_role_tenant_del` (`tenant_id`, `del_flag`, `id`),",
        ),
        (
            "sys_file",
            "    KEY `idx_tenant_id` (`tenant_id`),",
            "    KEY `idx_tenant_id` (`tenant_id`),\n    KEY `idx_file_tenant_del_size` (`tenant_id`, `del_flag`, `file_size`),",
        ),
        (
            "sys_job_schedule",
            "    KEY `idx_job_schedule_tenant` (`tenant_id`, `del_flag`, `created_at`)",
            "    KEY `idx_job_schedule_tenant` (`tenant_id`, `del_flag`, `created_at`),\n    KEY `idx_schedule_tenant_del_enabled` (`tenant_id`, `enabled`, `del_flag`)",
        ),
    ];
    for (table, needle, replacement) in replacements {
        if statement
            .trim_start()
            .starts_with(&format!("CREATE TABLE IF NOT EXISTS `{table}`"))
        {
            let current = statement.replacen(needle, replacement, 1);
            assert_ne!(
                current, statement,
                "租户容量治理审阅快照无法在 {table} 中安装规范索引"
            );
            return Cow::Owned(current);
        }
    }
    Cow::Borrowed(statement)
}
