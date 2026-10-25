//! 数据库错误输出的凭据遮蔽；不连接资源、不读取证书文件。

use ryframe_config::AppConfig;

/// 遮蔽所有已配置数据库角色及初始化凭据，不改变底层操作或错误分类。
pub fn redact(config: &AppConfig, message: &str) -> String {
    let mut secrets = configured_secrets(config);
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    secrets.dedup();
    // 先遮蔽完整 URL，避免特殊凭据替换改变 URI 分隔符后漏判连接地址。
    let mut redacted = message
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                "<连接地址已隐藏>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    for secret in secrets {
        redacted = redacted.replace(&secret, "<已隐藏>");
    }
    if redacted.contains("-----BEGIN") && redacted.contains("PRIVATE KEY-----") {
        return "数据库诊断包含私钥材料，原文已隐藏；请核对所选配置的 TLS 字段及私钥格式".into();
    }
    redacted
}

fn collect_secret(secrets: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    secrets.push(value.to_owned());
    secrets.push(value.replace('\\', "\\\\").replace('\'', "\\'"));
    secrets.push(value.replace('\'', "''"));
    let escaped = format!("{value:?}");
    secrets.push(escaped[1..escaped.len() - 1].to_owned());
}

fn configured_secrets(config: &AppConfig) -> Vec<String> {
    let mut secrets = Vec::new();
    for connection in std::iter::once(&config.database.primary)
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
    {
        connection_secrets(&mut secrets, connection);
    }
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
            connection_secrets(&mut secrets, &connection);
        }
    }
    if let Some(password) = &config.bootstrap.admin_password {
        collect_secret(&mut secrets, password);
    }
    secrets
}

fn connection_secrets(secrets: &mut Vec<String>, connection: &ryframe_config::DbConnection) {
    for value in [
        Some(connection.username.as_str()),
        Some(connection.password.as_str()),
        connection.tls_ca.as_deref(),
        connection.tls_client_cert.as_deref(),
        connection.tls_client_key.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    {
        collect_secret(secrets, value);
    }
    let url = connection.connection_url();
    if let Some((credentials, _)) = url
        .strip_prefix("mysql://")
        .and_then(|value| value.split_once('@'))
    {
        secrets.extend(
            credentials
                .split(':')
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        );
    }
    if let Some((_, query)) = url.split_once('?') {
        secrets.extend(
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .filter(|(key, _)| matches!(*key, "ssl-ca" | "ssl-cert" | "ssl-key"))
                .map(|(_, value)| value.to_owned()),
        );
    }
}
