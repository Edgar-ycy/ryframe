use ryframe_config::{
    DbTlsMode, SqlLogLevel, TenantDataConfig, TenantDatabaseTargetConfig, TenantDatabaseTargetKind,
    TenantDatabaseTargetMode,
};

use super::{TenantDataError, TenantDatabaseTargetRegistry, support::target_definition};

pub(super) const PASSWORD: &str = r"  tenant!@:/?%${NOT_EXPANDED}\密钥  ";

pub(super) fn target(password: Option<&str>) -> TenantDatabaseTargetConfig {
    TenantDatabaseTargetConfig {
        key: "private-target".into(),
        kind: TenantDatabaseTargetKind::Mysql,
        mode: TenantDatabaseTargetMode::Dedicated,
        max_connections: Some(8),
        host: Some("private-host.invalid".into()),
        port: Some(3307),
        database: Some("private_database".into()),
        username: Some("private_database_user".into()),
        password: password.map(str::to_owned),
        tls_mode: Some(DbTlsMode::VerifyIdentity),
        tls_ca: Some("private-ca.pem".into()),
        tls_client_cert: Some("private-client.pem".into()),
        tls_client_key: Some("private-key.pem".into()),
        ..TenantDatabaseTargetConfig::shared_control()
    }
}

#[test]
fn configured_password_is_owned_literal_and_keeps_tls_and_pool_settings() {
    let mut configured = target(Some(PASSWORD));
    let (_, definition) = target_definition(&configured, 4).unwrap();
    configured.password = Some("changed-after-registration".into());
    let mysql = definition.mysql.as_ref().unwrap();
    assert!(
        mysql.password == PASSWORD,
        "密码必须按配置原样克隆，不进行插值或修剪"
    );
    assert_eq!(mysql.host, "private-host.invalid");
    assert_eq!(mysql.port, 3307);
    assert_eq!(mysql.database, "private_database");
    assert_eq!(mysql.username, "private_database_user");
    assert_eq!(mysql.tls_mode, DbTlsMode::VerifyIdentity);
    assert_eq!(mysql.tls_ca.as_deref(), Some("private-ca.pem"));
    assert_eq!(mysql.tls_client_cert.as_deref(), Some("private-client.pem"));
    assert_eq!(mysql.tls_client_key.as_deref(), Some("private-key.pem"));
    assert_eq!(definition.pool_max_connections, Some(8));
}

#[test]
fn explicit_empty_password_is_valid_but_missing_password_fails_closed() {
    let (_, definition) = target_definition(&target(Some("")), 4).unwrap();
    assert!(definition.mysql.as_ref().unwrap().password.is_empty());
    let config = TenantDataConfig {
        targets: vec![target(Some(""))],
        ..Default::default()
    };
    assert!(TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200).is_ok());
    let error = target_definition(&target(None), 4).unwrap_err();
    assert!(matches!(error, TenantDataError::InvalidConfiguration(_)));
    let config = TenantDataConfig {
        targets: vec![target(None)],
        ..Default::default()
    };
    assert!(matches!(
        TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200),
        Err(TenantDataError::InvalidConfiguration(_))
    ));
}

#[tokio::test]
async fn registry_debug_and_metadata_do_not_expose_connection_secrets() {
    let config = TenantDataConfig {
        targets: vec![target(Some(PASSWORD))],
        max_open_targets: 2,
        max_total_connections: 16,
        idle_pool_secs: 37,
        ..Default::default()
    };
    let registry = TenantDatabaseTargetRegistry::new(&config, SqlLogLevel::Off, 200).unwrap();
    let debug = format!("{registry:?}");
    let metadata = registry.metadata().await;
    let public = format!("{metadata:?}");
    for secret in [
        PASSWORD,
        "private-host.invalid",
        "private_database",
        "private_database_user",
        "private-ca.pem",
        "private-client.pem",
        "private-key.pem",
        "mysql://",
    ] {
        assert!(!debug.contains(secret), "注册表 Debug 不得输出连接凭据");
        assert!(!public.contains(secret), "目标元数据不得输出连接凭据");
    }
    let mysql = metadata
        .iter()
        .find(|target| target.key == "private-target")
        .unwrap();
    assert!(!mysql.connected);
    assert_eq!(mysql.pool_max_connections, Some(8));
    assert_eq!(mysql.active_leases, 0);
    let stats = registry.pool_stats().await;
    assert_eq!(stats.max_total_connections, 16);
    assert_eq!(stats.reserved_connections, 0);
    assert_eq!(
        (
            stats.open_targets,
            stats.opening_targets,
            stats.active_leases
        ),
        (0, 0, 0)
    );
}

#[test]
fn safe_cause_redacts_escaped_credentials_urls_and_private_keys() {
    use std::error::Error;
    let config = ryframe_config::DbConnection {
        username: "private-user".into(),
        password: "p'\\ass@word".into(),
        tls_client_key: Some("private-key.pem".into()),
        ..Default::default()
    };
    let raw = format!(
        "1045 {} {:?} {} {}",
        config.password,
        config.password,
        config.connection_url(),
        config.tls_client_key.as_deref().unwrap()
    );
    let source = crate::error::SafeDatabaseCause::from_connection(&config, &raw);
    let error = TenantDataError::TargetUnavailable {
        target_key: "safe".into(),
        source: Some(source),
    };
    for output in [format!("{error:?}"), error.source().unwrap().to_string()] {
        assert!(output.contains("1045"));
        for secret in [
            &config.password[..],
            "private-user",
            "private-key.pem",
            "mysql://",
        ] {
            assert!(!output.contains(secret));
        }
    }
    let pem = crate::error::SafeDatabaseCause::from_connection(
        &config,
        "-----BEGIN PRIVATE KEY----- private-material",
    );
    assert!(!pem.to_string().contains("private-material"));
}
