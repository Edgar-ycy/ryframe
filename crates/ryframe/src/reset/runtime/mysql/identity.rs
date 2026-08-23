use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use ryframe_config::{AppConfig, DbConnection, DbTlsMode, TenantDatabaseTargetKind};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TryGetable};

use crate::reset::{
    ResetError, ResetResult,
    model::{
        PRIMARY_DATABASE_PASSWORD_ENV, ResetManifest, database_connection_identity, normalize_host,
        sha256_hex,
    },
};

use super::{ADMIN_PASSWORD_ENV, DatabaseHandle, DatabaseSpec, SeedCredentials, USER_PASSWORD_ENV};

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
    let mut specs = BTreeMap::<(String, u16, String), DatabaseSpec>::new();
    let primary_password = std::env::var(PRIMARY_DATABASE_PASSWORD_ENV)
        .map_err(|_| ResetError::new("控制库密码环境变量缺失或编码无效"))?;
    if primary_password != config.database.primary.password {
        return Err(ResetError::new(
            "控制库密码必须来自当前 APP_DATABASE_PASSWORD 环境变量",
        ));
    }
    let mut primary = config.database.primary.clone();
    primary.password = primary_password;
    insert_spec(&mut specs, primary, PRIMARY_DATABASE_PASSWORD_ENV, manifest)?;
    for target in &config.tenant_data.targets {
        if target.kind != TenantDatabaseTargetKind::Mysql {
            continue;
        }
        let password_env = target
            .password_env
            .as_deref()
            .ok_or_else(|| ResetError::new("MySQL 目标缺少 password_env"))?;
        let password = std::env::var(password_env)
            .map_err(|_| ResetError::new("MySQL 目标密码环境变量缺失或编码无效"))?;
        if password.is_empty() {
            return Err(ResetError::new("MySQL 目标密码环境变量不能为空"));
        }
        let connection = DbConnection {
            host: target
                .host
                .clone()
                .ok_or_else(|| ResetError::new("MySQL 目标缺少 host"))?,
            port: target.port.unwrap_or(3306),
            database: target
                .database
                .clone()
                .ok_or_else(|| ResetError::new("MySQL 目标缺少 database"))?,
            username: target
                .username
                .clone()
                .ok_or_else(|| ResetError::new("MySQL 目标缺少 username"))?,
            password,
            max_connections: target.max_connections.unwrap_or(2).min(4),
            min_connections: 0,
            acquire_timeout_secs: 10,
            idle_timeout_secs: 60,
            max_lifetime_secs: 300,
            connect_timeout_secs: 10,
            tls_mode: target.tls_mode.unwrap_or(DbTlsMode::Required),
            tls_ca: target.tls_ca.clone(),
            tls_client_cert: target.tls_client_cert.clone(),
            tls_client_key: target.tls_client_key.clone(),
        };
        insert_spec(&mut specs, connection, password_env, manifest)?;
    }
    if specs.len() != manifest.databases.len() {
        return Err(ResetError::new(
            "MySQL 运行时连接集合与不可变数据库清单不一致",
        ));
    }
    Ok(specs.into_values().collect())
}

fn insert_spec(
    specs: &mut BTreeMap<(String, u16, String), DatabaseSpec>,
    connection: DbConnection,
    password_env: &str,
    manifest: &ResetManifest,
) -> ResetResult<()> {
    let identity = (
        normalize_host(&connection.host),
        connection.port,
        connection.database.trim().to_owned(),
    );
    let resource = manifest
        .databases
        .iter()
        .find(|database| {
            database.host == identity.0
                && database.port == identity.1
                && database.database == identity.2
        })
        .ok_or_else(|| ResetError::new("MySQL 连接不属于不可变数据库清单"))?;
    if database_connection_identity(&connection, password_env)? != resource.connection {
        return Err(ResetError::new(
            "MySQL 非秘密连接参数与不可变清单不一致，请重新运行 plan",
        ));
    }
    if let Some(existing) = specs.get(&identity) {
        if !same_credentials(&existing.connection, &connection) {
            return Err(ResetError::new(
                "同一物理数据库配置了冲突的 MySQL 凭据或 TLS 参数",
            ));
        }
        return Ok(());
    }
    specs.insert(
        identity,
        DatabaseSpec {
            resource: resource.clone(),
            connection,
        },
    );
    Ok(())
}

pub fn same_credentials(left: &DbConnection, right: &DbConnection) -> bool {
    left.username == right.username
        && left.password == right.password
        && left.tls_mode == right.tls_mode
        && left.tls_ca == right.tls_ca
        && left.tls_client_cert == right.tls_client_cert
        && left.tls_client_key == right.tls_client_key
}

pub(super) fn load_seed_credentials() -> ResetResult<SeedCredentials> {
    fn load(name: &str) -> ResetResult<(String, String)> {
        let password = std::env::var(name)
            .map_err(|_| ResetError::new("reset 种子密码环境变量缺失或编码无效"))?;
        ryframe_auth::password::validate_complexity(&password)
            .map_err(|_| ResetError::new("reset 种子密码不满足复杂度策略"))?;
        let hash = ryframe_auth::password::hash(&password)
            .map_err(|_| ResetError::new("reset 种子密码哈希失败"))?;
        Ok((password, hash))
    }
    let (admin_password, admin_hash) = load(ADMIN_PASSWORD_ENV)?;
    let (user_password, user_hash) = load(USER_PASSWORD_ENV)?;
    Ok(SeedCredentials {
        admin_password,
        admin_hash,
        user_password,
        user_hash,
    })
}
