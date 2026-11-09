//! 数据库错误输出的凭据遮蔽；不连接资源、不读取证书文件。

use ryframe_config::AppConfig;

/// 枚举应用各数据库角色及初始化凭据，复用连接层脱敏规则。
pub fn redact(config: &AppConfig, message: &str) -> String {
    let mut connections = std::iter::once(&config.database.primary)
        .chain(
            config
                .database
                .replicas
                .iter()
                .map(|replica| &replica.connection),
        )
        .chain(
            config
                .database
                .sources
                .iter()
                .map(|source| &source.connection),
        )
        .cloned()
        .collect::<Vec<_>>();
    for target in &config.tenant_data.targets {
        if target.kind == ryframe_config::TenantDatabaseTargetKind::Mysql {
            // 仅复用正式 URL 编码；不创建连接或读取 TLS 文件。
            let connection = ryframe_config::DbConnection {
                host: target.host.clone().unwrap_or_default(),
                port: target.port.unwrap_or(3306),
                database: target.database.clone().unwrap_or_default(),
                username: target.username.clone().unwrap_or_default(),
                password: target.password.clone().unwrap_or_default(),
                tls_mode: target.tls_mode.unwrap_or_default(),
                tls_ca: target.tls_ca.clone(),
                tls_client_cert: target.tls_client_cert.clone(),
                tls_client_key: target.tls_client_key.clone(),
                ..ryframe_config::DbConnection::default()
            };
            connections.push(connection);
        }
    }
    ryframe_db::connection::redact_database_diagnostic(
        &connections,
        config.bootstrap.admin_password.as_deref(),
        message,
    )
}
