use super::*;
use ryframe_config::{DatabaseReplicaConfig, DatabaseSourceConfig};

#[test]
fn required_database_diagnostic_context_is_preserved() {
    let config = diagnostic_configuration();

    for (connection, field, name) in [
        (&config.database.primary, "database.primary", None),
        (
            &config.database.sources[0].connection,
            "database.sources[0]",
            Some("legacy"),
        ),
    ] {
        for phase in ["连接池建立", "PING 健康检查"] {
            let actual = startup_database_error(
                &config,
                connection,
                field,
                name,
                phase,
                AppError::Database("pool timed out while waiting for an open connection".into()),
            )
            .to_string();
            for expected in [
                format!("阶段={phase}"),
                format!("字段={field}"),
                format!("name={}", name.unwrap_or("primary")),
                "目标=[127.0.0.1]:13306/diagnostic_scope".into(),
                "pool timed out while waiting for an open connection".into(),
                "本次进程启动已中止，尚未进入服务监听阶段".into(),
                "服务监听与网络可达".into(),
                "账号连接及数据库访问权限".into(),
                "tls_mode".into(),
                "connect_timeout_secs=7".into(),
                "acquire_timeout_secs=11".into(),
                "修复后重新启动".into(),
            ] {
                assert!(actual.contains(&expected), "必要诊断丢失：{expected}");
            }
        }
    }
}

#[test]
fn required_database_diagnostics_hide_credentials_and_uris() {
    let config = diagnostic_configuration();
    let mut forbidden = Vec::new();
    for connection in std::iter::once(&config.database.primary)
        .chain(
            config
                .database
                .sources
                .iter()
                .map(|source| &source.connection),
        )
        .chain(
            config
                .database
                .replicas
                .iter()
                .map(|replica| &replica.connection),
        )
    {
        for value in [
            &connection.username,
            &connection.password,
            connection.tls_ca.as_ref().unwrap(),
            connection.tls_client_cert.as_ref().unwrap(),
            connection.tls_client_key.as_ref().unwrap(),
        ] {
            forbidden.push(value.clone());
            let escaped = format!("{value:?}");
            forbidden.push(escaped[1..escaped.len() - 1].to_owned());
            forbidden.push(value.replace('\\', "\\\\").replace('\'', "\\'"));
            forbidden.push(value.replace('\'', "''"));
        }
        let url = connection.connection_url();
        let (credentials, _) = url
            .strip_prefix("mysql://")
            .unwrap()
            .split_once('@')
            .unwrap();
        forbidden.extend(credentials.split(':').map(str::to_owned));
        let (_, query) = url.split_once('?').unwrap();
        forbidden.extend(
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .filter(|(key, _)| matches!(*key, "ssl-ca" | "ssl-cert" | "ssl-key"))
                .map(|(_, value)| value.to_owned()),
        );
        forbidden.push(url);
    }
    let actual = startup_database_error(
        &config,
        &config.database.primary,
        "database.primary",
        None,
        "连接池建立",
        AppError::Database(format!(
            "access denied: {} other=mysql://unlisted:synthetic@localhost/db",
            forbidden.join(" | ")
        )),
    )
    .to_string();
    for secret in forbidden {
        assert!(!actual.contains(&secret), "敏感诊断未被隐藏");
    }
    assert!(!actual.contains("://"));
    assert!(!actual.contains("unlisted:synthetic"));
    assert!(actual.contains("access denied"));
    assert!(actual.contains("database.primary"));
    assert!(actual.contains("修复后重新启动"));
}

#[test]
fn required_database_diagnostics_hide_pem_and_keep_repair_guidance() {
    let config = diagnostic_configuration();
    let pem = "-----BEGIN PRIVATE KEY-----\nSYNTHETIC-KEY-MATERIAL\n-----END PRIVATE KEY-----";
    let actual = startup_database_error(
        &config,
        &config.database.sources[0].connection,
        "database.sources[0]",
        Some("legacy"),
        "PING 健康检查",
        AppError::Database(pem.into()),
    )
    .to_string();
    for marker in ["-----BEGIN", "-----END", "SYNTHETIC-KEY-MATERIAL"] {
        assert!(!actual.contains(marker));
    }
    assert!(actual.contains("database.sources[0]"));
    assert!(actual.contains("PING 健康检查"));
    assert!(actual.contains("tls_mode"));
    assert!(actual.contains("修复后重新启动"));
    let fallback = redact_database_diagnostic(&config, pem);
    assert!(!fallback.contains("SYNTHETIC-KEY-MATERIAL"));
    assert!(fallback.contains("TLS"));
    assert!(fallback.contains("重新启动"));
}

fn diagnostic_configuration() -> AppConfig {
    let mut config: AppConfig =
        toml::from_str(include_str!("../../../../config/app.toml")).unwrap();
    config.database.primary = DbConnection {
        host: "127.0.0.1".into(),
        port: 13306,
        database: "diagnostic_scope".into(),
        username: "synthetic_operator_94@ops".into(),
        password: "Synthetic'Secret\\42!".into(),
        tls_ca: Some("/synthetic/ca path.pem".into()),
        tls_client_cert: Some("/synthetic/client cert.pem".into()),
        tls_client_key: Some("D:\\synthetic\\private key.pem".into()),
        connect_timeout_secs: 7,
        acquire_timeout_secs: 11,
        ..DbConnection::default()
    };
    let mut source = config.database.primary.clone();
    source.username = "synthetic_source_52".into();
    source.password = "Synthetic-Source#83".into();
    config.database.sources = vec![DatabaseSourceConfig {
        name: "legacy".into(),
        connection: source,
    }];
    let mut replica = config.database.primary.clone();
    replica.username = "synthetic_replica_36".into();
    replica.password = "Synthetic-Replica!61".into();
    config.database.replicas = vec![DatabaseReplicaConfig {
        name: "reader".into(),
        connection: replica,
    }];

    config
}

#[test]
fn shared_database_redaction_also_covers_tenant_target_encodings() {
    let mut config = diagnostic_configuration();
    let target: ryframe_config::TenantDatabaseTargetConfig = toml::from_str(
        r#"
key = "diagnostic-tenant"
kind = "mysql"
mode = "dedicated"
host = "127.0.0.1"
port = 3306
database = "diagnostic_scope"
username = "tenant_operator_81@ops"
password = "Tenant'Private\\42!"
tls_mode = "verify_identity"
tls_ca = "/tenant/ca path.pem"
tls_client_cert = "/tenant/client cert.pem"
tls_client_key = "D:\\tenant\\private key.pem"
"#,
    )
    .unwrap();
    let connection = DbConnection {
        username: target.username.clone().unwrap(),
        password: target.password.clone().unwrap(),
        tls_ca: target.tls_ca.clone(),
        tls_client_cert: target.tls_client_cert.clone(),
        tls_client_key: target.tls_client_key.clone(),
        ..DbConnection::default()
    };
    config.tenant_data.targets.push(target);
    let url = connection.connection_url();
    let mut forbidden = vec![
        connection.username,
        connection.password,
        connection.tls_ca.unwrap(),
        connection.tls_client_cert.unwrap(),
        connection.tls_client_key.unwrap(),
    ];
    let (_, query) = url.split_once('?').unwrap();
    forbidden.extend(
        query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .filter(|(key, _)| matches!(*key, "ssl-ca" | "ssl-cert" | "ssl-key"))
            .map(|(_, value)| value.to_owned()),
    );
    let source = format!("access denied {} {url}", forbidden.join(" | "));
    let redacted = crate::database_diagnostic::redact(&config, &source);
    for value in forbidden {
        assert!(!redacted.contains(&value), "租户目标凭据未隐藏");
    }
    assert!(!redacted.contains("://"));
    assert!(redacted.contains("access denied"));
}
