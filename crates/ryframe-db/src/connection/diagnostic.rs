//! 连接诊断脱敏；只处理显式配置，不读取文件或连接资源。

/// 按实际连接与额外秘密遮蔽诊断，不保留原始错误。
pub fn redact_database_diagnostic<'a>(
    connections: impl IntoIterator<Item = &'a ryframe_config::DbConnection>,
    extra_secrets: impl IntoIterator<Item = &'a str>,
    message: &str,
) -> String {
    let mut secrets = Vec::new();
    for connection in connections {
        connection_secrets(&mut secrets, connection);
    }
    for secret in extra_secrets {
        collect_secret(&mut secrets, secret);
    }
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
