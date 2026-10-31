//! 连接诊断脱敏；只处理显式配置，不读取文件或连接资源。

/// 按实际连接与额外秘密遮蔽诊断，不保留原始错误。
pub fn redact_database_diagnostic<'a>(
    connections: impl IntoIterator<Item = &'a ryframe_config::DbConnection>,
    extra_secrets: impl IntoIterator<Item = &'a str>,
    message: &str,
) -> String {
    // 先识别原始私钥标记，避免秘密遮蔽抹掉标记后留下未登记的正文。
    if contains_private_key(message) {
        return "数据库诊断包含私钥材料，原文已隐藏；请核对所选配置的 TLS 字段及私钥格式".into();
    }
    let mut secrets = Vec::new();
    for connection in connections {
        connection_secrets(&mut secrets, connection);
    }
    for secret in extra_secrets {
        collect_secret(&mut secrets, secret);
    }
    secrets.sort_unstable();
    secrets.dedup();
    // 秘密和 URI 都定位在原文上，合并后一次输出，不让先后替换拆散秘密。
    let mut redacted = String::with_capacity(message.len());
    let mut cursor = 0;
    for range in redaction_ranges(message, &secrets) {
        redacted.push_str(&message[cursor..range.start]);
        redacted.push_str(if range.address {
            "<连接地址已隐藏>"
        } else {
            "<已隐藏>"
        });
        cursor = range.end;
    }
    redacted.push_str(&message[cursor..]);
    redacted.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct RedactionRange {
    start: usize,
    end: usize,
    address: bool,
}

fn redaction_ranges(message: &str, secrets: &[String]) -> Vec<RedactionRange> {
    let mut ranges = Vec::new();
    for secret in secrets {
        let step = secret.chars().next().expect("脱敏秘密必须非空").len_utf8();
        let mut cursor = 0;
        while let Some(offset) = message[cursor..].find(secret) {
            let start = cursor + offset;
            ranges.push(RedactionRange {
                start,
                end: start + secret.len(),
                address: false,
            });
            // 按字符推进，保留同一秘密重叠出现的区间及 UTF-8 边界。
            cursor = start + step;
        }
    }
    let mut cursor = 0;
    for token in message.split_whitespace() {
        let start = cursor + message[cursor..].find(token).expect("诊断词元来自原文");
        let end = start + token.len();
        cursor = end;
        if token.contains("://") {
            ranges.push(RedactionRange {
                start,
                end,
                address: true,
            });
        }
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<RedactionRange> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
            previous.address |= range.address;
        } else {
            merged.push(range);
        }
    }
    merged
}

fn contains_private_key(message: &str) -> bool {
    message.contains("-----BEGIN")
        && message
            .split_whitespace()
            .zip(message.split_whitespace().skip(1))
            .any(|(left, right)| left.ends_with("PRIVATE") && right.starts_with("KEY-----"))
}

fn collect_secret(secrets: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    // 只扩展诊断匹配形式；连接配置和真实密码不做 trim 或空白改写。
    for literal in [
        value.to_owned(),
        value.split_whitespace().collect::<Vec<_>>().join(" "),
    ] {
        if literal.is_empty() {
            continue;
        }
        secrets.push(literal.clone());
        secrets.push(literal.replace('\\', "\\\\").replace('\'', "\\'"));
        secrets.push(literal.replace('\'', "''"));
        let escaped = format!("{literal:?}");
        secrets.push(escaped[1..escaped.len() - 1].to_owned());
    }
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
