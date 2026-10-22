//! 先核对全部别名实际凭据，再为每个物理数据库选择唯一非秘密配置引用。

use std::collections::BTreeMap;

use ryframe_config::{
    AppConfig, DbConnection, TenantDatabaseTargetConfig, TenantDatabaseTargetKind,
};

use super::{
    ResetError, ResetResult,
    model::{PRIMARY_DATABASE_PASSWORD_CONFIG_KEY, normalize_host},
};

pub(super) struct ConfiguredDatabase {
    pub connection: DbConnection,
    pub password_config_key: String,
    pub target_keys: Vec<String>,
    pub control_baseline: bool,
}

pub(super) fn configured_databases(config: &AppConfig) -> ResetResult<Vec<ConfiguredDatabase>> {
    let mut databases = BTreeMap::new();
    insert(
        &mut databases,
        config.database.primary.clone(),
        PRIMARY_DATABASE_PASSWORD_CONFIG_KEY.into(),
        "shared-control",
        true,
    )?;
    let mut targets = config
        .tenant_data
        .targets
        .iter()
        .filter(|target| target.kind == TenantDatabaseTargetKind::Mysql)
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.key.cmp(&right.key));
    for target in targets {
        let connection = target_connection(target)?;
        let reference = format!("tenant_data.targets.{}.password", target.key);
        insert(&mut databases, connection, reference, &target.key, false)?;
    }
    Ok(databases.into_values().collect())
}

fn insert(
    databases: &mut BTreeMap<(String, u16, String), ConfiguredDatabase>,
    connection: DbConnection,
    password_config_key: String,
    target_key: &str,
    control_baseline: bool,
) -> ResetResult<()> {
    let identity = (
        normalize_host(&connection.host),
        connection.port,
        connection.database.trim().to_owned(),
    );
    if let Some(existing) = databases.get_mut(&identity) {
        if !same_credentials(&existing.connection, &connection) {
            return Err(ResetError::new(
                "同一物理数据库配置了冲突的 MySQL 凭据或 TLS 参数",
            ));
        }
        if !existing.target_keys.iter().any(|key| key == target_key) {
            existing.target_keys.push(target_key.to_owned());
            existing.target_keys.sort_unstable();
        }
        return Ok(());
    }
    // 主库先登记，其他目标按 key 排序；首个引用就是确定性的 canonical 引用。
    databases.insert(
        identity,
        ConfiguredDatabase {
            connection,
            password_config_key,
            target_keys: vec![target_key.into()],
            control_baseline,
        },
    );
    Ok(())
}

fn target_connection(target: &TenantDatabaseTargetConfig) -> ResetResult<DbConnection> {
    Ok(DbConnection {
        host: target
            .host
            .clone()
            .ok_or_else(|| ResetError::new("MySQL 目标缺少 host 配置"))?,
        port: target.port.unwrap_or(3306),
        database: target
            .database
            .clone()
            .ok_or_else(|| ResetError::new("MySQL 目标缺少 database 配置"))?,
        username: target
            .username
            .clone()
            .ok_or_else(|| ResetError::new("MySQL 目标缺少 username 配置"))?,
        password: target.password.clone().ok_or_else(|| {
            ResetError::new("MySQL 目标缺少 password 配置字段；请显式填写，空密码使用空字符串")
        })?,
        max_connections: target.max_connections.unwrap_or(2).min(4),
        min_connections: 0,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 60,
        max_lifetime_secs: 300,
        connect_timeout_secs: 10,
        tls_mode: target.tls_mode.unwrap_or_default(),
        tls_ca: target.tls_ca.clone(),
        tls_client_cert: target.tls_client_cert.clone(),
        tls_client_key: target.tls_client_key.clone(),
    })
}

pub fn same_credentials(left: &DbConnection, right: &DbConnection) -> bool {
    left.username == right.username
        && left.password == right.password
        && left.tls_mode == right.tls_mode
        && left.tls_ca == right.tls_ca
        && left.tls_client_cert == right.tls_client_cert
        && left.tls_client_key == right.tls_client_key
}
