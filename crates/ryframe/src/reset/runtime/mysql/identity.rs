use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use ryframe_config::AppConfig;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TryGetable};

use crate::reset::{
    ResetError, ResetResult,
    model::{ResetManifest, database_connection_identity, normalize_host, sha256_hex},
};

use super::{DatabaseHandle, DatabaseSpec, SeedCredentials};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnershipState {
    Verified,
    Missing,
    Mismatch,
}

pub(super) async fn server_identity(db: &DatabaseConnection) -> ResetResult<(String, i64)> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@server_uuid, CAST(@@lower_case_table_names AS CHAR)",
        ))
        .await
        .map_err(|_| ResetError::new("无法读取 MySQL 物理 server 身份"))?
        .ok_or_else(|| ResetError::new("MySQL 物理 server 身份查询无结果"))?;
    let server_uuid = String::try_get_by_index(&row, 0)
        .map_err(|_| ResetError::new("MySQL server_uuid 格式无效"))?;
    let lower_case_table_names = String::try_get_by_index(&row, 1)
        .map_err(|_| ResetError::new("MySQL lower_case_table_names 格式无效"))?;
    let lower_case_table_names = parse_lower_case_table_names(&lower_case_table_names)?;
    if server_uuid.trim().is_empty()
        || server_uuid.len() > 64
        || !matches!(lower_case_table_names, 0..=2)
    {
        return Err(ResetError::new("MySQL 物理 server 身份值无效"));
    }
    Ok((server_uuid, lower_case_table_names))
}

pub fn parse_lower_case_table_names(value: &str) -> ResetResult<i64> {
    let parsed = value
        .parse::<i64>()
        .map_err(|_| ResetError::new("MySQL lower_case_table_names 格式无效"))?;
    if !matches!(parsed, 0..=2) {
        return Err(ResetError::new("MySQL lower_case_table_names 值无效"));
    }
    Ok(parsed)
}

pub(super) fn validate_physical_database_identities(handles: &[DatabaseHandle]) -> ResetResult<()> {
    let mut identities = BTreeSet::new();
    let mut server_modes = BTreeMap::new();
    for handle in handles {
        if let Some(existing) =
            server_modes.insert(handle.server_uuid.as_str(), handle.lower_case_table_names)
            && existing != handle.lower_case_table_names
        {
            return Err(ResetError::new(
                "同一 MySQL server 返回了不一致的大小写模式",
            ));
        }
        let database = if handle.lower_case_table_names == 0 {
            handle.resource.database.clone()
        } else {
            handle.resource.database.to_ascii_lowercase()
        };
        if !identities.insert((handle.server_uuid.as_str(), database)) {
            return Err(ResetError::new(
                "多个 host/DNS 配置指向同一 MySQL 物理数据库，拒绝重复重建",
            ));
        }
    }
    Ok(())
}

pub(super) fn database_physical_identity(handle: &DatabaseHandle) -> String {
    let database = if handle.lower_case_table_names == 0 {
        Cow::Borrowed(handle.resource.database.as_str())
    } else {
        Cow::Owned(handle.resource.database.to_ascii_lowercase())
    };
    sha256_hex(format!("mysql:{}:{database}", handle.server_uuid).as_bytes())
}

pub(super) fn collect_specs(
    config: &AppConfig,
    manifest: &ResetManifest,
) -> ResetResult<Vec<DatabaseSpec>> {
    let mut specs = Vec::new();
    for configured in crate::reset::connections::configured_databases(config)? {
        let connection = configured.connection;
        let resource = manifest
            .databases
            .iter()
            .find(|database| {
                database.host == normalize_host(&connection.host)
                    && database.port == connection.port
                    && database.database == connection.database.trim()
            })
            .ok_or_else(|| ResetError::new("MySQL 连接不属于不可变数据库清单"))?;
        if database_connection_identity(&connection, &configured.password_config_key)?
            != resource.connection
            || resource.target_keys != configured.target_keys
            || resource.control_baseline != configured.control_baseline
            || !resource.tenant_baseline
        {
            return Err(ResetError::new(
                "MySQL 配置引用、连接身份或别名集合与不可变清单不一致，请重新运行 plan",
            ));
        }
        specs.push(DatabaseSpec {
            resource: resource.clone(),
            connection,
        });
    }
    if specs.len() != manifest.databases.len() {
        return Err(ResetError::new(
            "MySQL 运行时连接集合与不可变数据库清单不一致",
        ));
    }
    Ok(specs)
}

pub(super) fn seed_credentials(admin_password: Option<&str>) -> ResetResult<SeedCredentials> {
    use ryframe_auth::password;

    let admin_password = admin_password.ok_or_else(|| ResetError::new(
        "所选配置文件缺少 bootstrap.admin_password；请填写 8~72 个可见 ASCII 字符，包含大小写字母、数字和符号"
    ))?;
    password::validate_complexity(admin_password).map_err(|_| ResetError::new(
        "所选配置文件 bootstrap.admin_password 不满足复杂度策略；请填写 8~72 个可见 ASCII 字符，包含大小写字母、数字和符号"
    ))?;
    let admin_hash =
        password::hash(admin_password).map_err(|_| ResetError::new("reset 管理员密码哈希失败"))?;
    let user_hash = password::hash(password::INITIAL_USER_PASSWORD)
        .map_err(|_| ResetError::new("reset 普通用户初始密码哈希失败"))?;
    Ok(SeedCredentials {
        admin_password: admin_password.to_owned(),
        admin_hash,
        user_hash,
    })
}
