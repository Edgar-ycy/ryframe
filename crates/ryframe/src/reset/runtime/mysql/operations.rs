use std::collections::BTreeSet;

use ryframe_config::DbConnection;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement, TryGetable,
};

use crate::reset::{ResetError, ResetResult, model::PhysicalDatabase};

use super::{
    DatabaseHandle, EMPTY_CONTROL_TABLES, REQUIRED_GLOBAL_PRIVILEGES, SeedCredentials,
    identity::OwnershipState,
};

pub(super) async fn connect_target(
    config: &DbConnection,
    resource: &PhysicalDatabase,
) -> ResetResult<DatabaseConnection> {
    ryframe_db::connection::connect(config).await.map_err(|_| {
        ResetError::new(format!(
            "无法连接目标数据库 {}:{}/{}",
            resource.host, resource.port, resource.database
        ))
    })
}

pub(super) async fn database_exists<C>(db: &C, name: &str) -> ResetResult<bool>
where
    C: ConnectionTrait + ?Sized,
{
    Ok(scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
        [name.into()],
    )
    .await?
        == Some(1))
}

pub(super) async fn verify_database_ownership_before_recreate(
    handle: &DatabaseHandle,
    server: &DatabaseTransaction,
    legacy_exclusive: bool,
    resource_started: bool,
) -> ResetResult<()> {
    if !database_exists(server, &handle.resource.database).await? {
        return if legacy_exclusive || resource_started {
            Ok(())
        } else {
            Err(ResetError::new(format!(
                "数据库 {} 不存在且无法验证所有权；仅允许显式独占接管或同 manifest 续跑",
                handle.resource.database
            )))
        };
    }
    let ownership = inspect_database_ownership_on_server(server, &handle.resource).await?;
    match ownership {
        OwnershipState::Verified => Ok(()),
        OwnershipState::Missing if legacy_exclusive || resource_started => Ok(()),
        OwnershipState::Missing => Err(ResetError::new(format!(
            "数据库 {} 缺少 scope marker；仅能通过明确的 dev/test 旧资源独占配置接管",
            handle.resource.database
        ))),
        OwnershipState::Mismatch => Err(ResetError::new(format!(
            "数据库 {} 的 scope marker 不匹配",
            handle.resource.database
        ))),
    }
}

pub(super) async fn verify_server_writable(db: &DatabaseConnection) -> ResetResult<()> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@global.read_only, @@global.super_read_only",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 只读状态"))?
        .ok_or_else(|| ResetError::new("MySQL 只读状态查询无结果"))?;
    let read_only = i64::try_get_by_index(&row, 0)
        .map_err(|_| ResetError::new("MySQL read_only 状态格式无效"))?;
    let super_read_only = i64::try_get_by_index(&row, 1)
        .map_err(|_| ResetError::new("MySQL super_read_only 状态格式无效"))?;
    if read_only != 0 || super_read_only != 0 {
        return Err(ResetError::new("MySQL server 为只读目标，永久拒绝重建"));
    }
    Ok(())
}

pub(super) async fn verify_global_privileges(db: &DatabaseConnection) -> ResetResult<()> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SHOW GRANTS FOR CURRENT_USER()",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 当前账号权限"))?;
    let mut granted = BTreeSet::new();
    let mut all = false;
    for row in rows {
        let grant = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL grant 返回格式无效"))?
            .to_ascii_uppercase();
        let Some((prefix, _)) = grant.split_once(" ON *.*") else {
            continue;
        };
        let list = prefix.strip_prefix("GRANT ").unwrap_or_default();
        if list == "ALL PRIVILEGES" {
            all = true;
            break;
        }
        granted.extend(list.split(',').map(str::trim).map(str::to_owned));
    }
    if !all
        && REQUIRED_GLOBAL_PRIVILEGES
            .iter()
            .any(|privilege| !granted.contains(*privilege))
    {
        return Err(ResetError::new(
            "MySQL 当前账号缺少重建所需的全局 DDL/DML 权限",
        ));
    }
    Ok(())
}

async fn inspect_database_ownership_on_server<C>(
    db: &C,
    resource: &PhysicalDatabase,
) -> ResetResult<OwnershipState>
where
    C: ConnectionTrait + ?Sized,
{
    let table_exists = scalar_i64(
        db,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = ? AND table_name = 'ryframe_resource_ownership'",
        [resource.database.as_str().into()],
    )
    .await?
        == Some(1);
    if !table_exists {
        return Ok(OwnershipState::Missing);
    }
    let database = quote_identifier(&resource.database)?;
    let sql =
        format!("SELECT marker FROM {database}.ryframe_resource_ownership WHERE resource_kind = ?");
    for (kind, expected) in &resource.ownership_markers {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::MySql,
                &sql,
                [kind.as_str().into()],
            ))
            .await
            .map_err(|_| ResetError::new("无法读取 MySQL 所有权 marker"))?;
        let Some(row) = row else {
            return Ok(OwnershipState::Missing);
        };
        let actual = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL 所有权 marker 格式无效"))?;
        if actual != *expected {
            return Ok(OwnershipState::Mismatch);
        }
    }
    Ok(OwnershipState::Verified)
}

pub(super) async fn write_seed_hashes(
    db: &DatabaseConnection,
    seeds: &SeedCredentials,
) -> ResetResult<()> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "UPDATE sys_user SET password_hash = CASE username WHEN 'admin' THEN ? WHEN 'user' THEN ? ELSE password_hash END WHERE username IN ('admin', 'user')",
            [
                seeds.admin_hash.as_str().into(),
                seeds.user_hash.as_str().into(),
            ],
        ))
        .await
        .map_err(|_| ResetError::new("写入环境提供的种子密码失败"))?;
    if result.rows_affected() != 2 {
        return Err(ResetError::new("种子账号数量不符合唯一 baseline 预期"));
    }
    Ok(())
}

pub(super) async fn verify_seed_hashes(
    db: &DatabaseConnection,
    seeds: &SeedCredentials,
) -> ResetResult<()> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT username, password_hash FROM sys_user WHERE username IN ('admin', 'user') ORDER BY username",
        ))
        .await
        .map_err(|_| ResetError::new("读取种子账号验证信息失败"))?;
    if rows.len() != 2 {
        return Err(ResetError::new("种子账号验证数量不匹配"));
    }
    for row in rows {
        let username = String::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("种子账号验证格式无效"))?;
        let actual = String::try_get_by_index(&row, 1)
            .map_err(|_| ResetError::new("种子密码验证格式无效"))?;
        let password = match username.as_str() {
            "admin" => &seeds.admin_password,
            "user" => &seeds.user_password,
            _ => return Err(ResetError::new("出现未知种子账号")),
        };
        if !ryframe_auth::password::verify(password, &actual)
            .map_err(|_| ResetError::new("种子密码验证失败"))?
        {
            return Err(ResetError::new("环境种子密码未按预期持久化"));
        }
    }
    Ok(())
}

pub(super) async fn verify_empty_control_resources(db: &DatabaseConnection) -> ResetResult<()> {
    for table in EMPTY_CONTROL_TABLES {
        let quoted = quote_identifier(table)?;
        let count = scalar_i64(db, &format!("SELECT COUNT(*) FROM {quoted}"), []).await?;
        if count != Some(0) {
            return Err(ResetError::new(format!(
                "控制库运行时资源表 {table} 在重建后非空"
            )));
        }
    }
    Ok(())
}

pub(super) async fn verify_empty_external_tenant(db: &DatabaseConnection) -> ResetResult<()> {
    if scalar_i64(db, "SELECT COUNT(*) FROM biz_tenant_fence", []).await? != Some(0) {
        return Err(ResetError::new("外部租户库 fence 在重建后非空"));
    }
    let occupied = scalar_i64(
        db,
        "SELECT COUNT(*) FROM biz_tenant_target_slot WHERE tenant_id IS NOT NULL OR placement_generation IS NOT NULL OR switch_token IS NOT NULL",
        [],
    )
    .await?;
    if occupied != Some(0) {
        return Err(ResetError::new("外部租户库 slot 在重建后被占用"));
    }
    for table in ryframe_tenant_db::migration::TENANT_DATA_CATALOG.tables() {
        let quoted = quote_identifier(table.table)?;
        if scalar_i64(db, &format!("SELECT COUNT(*) FROM {quoted}"), []).await? != Some(0) {
            return Err(ResetError::new("外部租户业务表在重建后非空"));
        }
    }
    Ok(())
}

pub fn quote_identifier(identifier: &str) -> ResetResult<String> {
    if identifier.is_empty()
        || identifier.len() > 64
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ResetError::new("拒绝引用不安全的 MySQL 标识符"));
    }
    Ok(format!("`{identifier}`"))
}

pub(super) async fn scalar_i64<C, const N: usize>(
    db: &C,
    sql: &str,
    values: [sea_orm::Value; N],
) -> ResetResult<Option<i64>>
where
    C: ConnectionTrait + ?Sized,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            sql,
            values,
        ))
        .await
        .map_err(|_| ResetError::new("MySQL 安全预检查询失败"))?;
    match row {
        Some(row) => Option::<i64>::try_get_by_index(&row, 0)
            .map_err(|_| ResetError::new("MySQL 标量查询格式无效")),
        None => Ok(None),
    }
}
