use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use ryframe_config::{AppConfig, DbConnection, Environment};

struct Fixture(PathBuf);

impl Fixture {
    fn new(collection: &str, edit: impl FnOnce(&mut toml::Table)) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local-tests/flattened-connection-contracts");
        fs::create_dir_all(&parent).unwrap();
        let directory = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let mut table: toml::Table =
            toml::from_str(include_str!("../../../config/app.toml")).unwrap();
        table["auth"]["jwt_secret"] = "PRIVATE-SENTINEL-auth".into();
        table["database"]["primary"]["password"] = "PRIVATE-SENTINEL-primary".into();
        let mut connection = table["database"]["primary"].as_table().unwrap().clone();
        connection.insert("name".into(), "first-connection".into());
        connection.insert("password".into(), "".into());
        let first = toml::Value::Table(connection.clone());
        connection.insert("name".into(), "second-connection".into());
        edit(&mut connection);
        table["database"].as_table_mut().unwrap().insert(
            collection.into(),
            toml::Value::Array(vec![first, toml::Value::Table(connection)]),
        );
        fs::write(directory.join("app.toml"), toml::to_string(&table).unwrap()).unwrap();
        fs::write(directory.join("app.dev.toml"), "").unwrap();
        Self(directory)
    }

    fn error(&self, collection: &str, field: &str, reason: &str) {
        let before = ["app.toml", "app.dev.toml"].map(|name| fs::read(self.0.join(name)).unwrap());
        let error = AppConfig::load(&self.0, Environment::Dev)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&self.0.display().to_string()), "{error}");
        assert!(error.contains("app.toml"), "{error}");
        assert!(error.contains("app.dev.toml"), "{error}");
        assert!(
            error.contains(&format!("database.{collection}[1].{field}")),
            "{error}"
        );
        assert!(error.contains(reason), "{error}");
        assert!(error.contains("请"), "{error}");
        assert!(error.contains("修正 TOML"), "{error}");
        assert!(!error.contains("PRIVATE-SENTINEL"), "{error}");
        assert!(
            !error.contains("invalid type:"),
            "不得保留原始 cause：{error}"
        );
        assert!(
            !error.contains("unknown variant"),
            "不得保留原始 cause：{error}"
        );
        let after = ["app.toml", "app.dev.toml"].map(|name| fs::read(self.0.join(name)).unwrap());
        assert_eq!(after, before);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn flattened_connection_type_errors_keep_the_exact_field_and_hide_values() {
    for collection in ["replicas", "sources"] {
        for (field, value) in [
            (
                "password",
                toml::Value::Array(vec!["PRIVATE-SENTINEL-password".into()]),
            ),
            ("host", 123.into()),
            ("port", "PRIVATE-SENTINEL-port".into()),
            (
                "tls_ca",
                toml::Value::Array(vec!["PRIVATE-SENTINEL-ca".into()]),
            ),
        ] {
            let fixture = Fixture::new(collection, |connection| {
                connection.insert(field.into(), value);
            });
            fixture.error(collection, field, "类型不正确");
        }
    }
}

#[test]
fn flattened_connection_unknown_fields_are_rejected_without_echoing_values() {
    for collection in ["replicas", "sources"] {
        let fixture = Fixture::new(collection, |connection| {
            connection.insert(
                "unsupported_option".into(),
                "PRIVATE-SENTINEL-unknown".into(),
            );
        });
        fixture.error(collection, "unsupported_option", "未知连接字段");
    }
}

#[test]
fn flattened_connection_missing_required_fields_keep_the_exact_field() {
    for collection in ["replicas", "sources"] {
        for field in [
            "host",
            "port",
            "database",
            "username",
            "password",
            "max_connections",
            "min_connections",
        ] {
            let fixture = Fixture::new(collection, |connection| {
                connection.remove(field);
            });
            fixture.error(collection, field, "缺少必填连接字段");
        }
    }
}

#[test]
fn flattened_connection_invalid_enum_values_are_hidden() {
    for collection in ["replicas", "sources"] {
        let fixture = Fixture::new(collection, |connection| {
            connection.insert("tls_mode".into(), "PRIVATE-SENTINEL-mode".into());
        });
        fixture.error(collection, "tls_mode", "枚举值不受支持");
    }
}

#[test]
fn flattened_connection_names_empty_passwords_and_defaults_remain_valid() {
    for collection in ["replicas", "sources"] {
        let fixture = Fixture::new(collection, |_| {});
        let before =
            ["app.toml", "app.dev.toml"].map(|name| fs::read(fixture.0.join(name)).unwrap());
        let config = AppConfig::load(&fixture.0, Environment::Dev).unwrap();
        let (name, connection) = if collection == "replicas" {
            (
                &config.database.replicas[1].name,
                &config.database.replicas[1].connection,
            )
        } else {
            (
                &config.database.sources[1].name,
                &config.database.sources[1].connection,
            )
        };
        assert_eq!(name, "second-connection");
        assert!(connection.password.is_empty());
        let defaults = DbConnection::default();
        assert_eq!(
            connection.acquire_timeout_secs,
            defaults.acquire_timeout_secs
        );
        assert_eq!(connection.idle_timeout_secs, defaults.idle_timeout_secs);
        assert_eq!(connection.max_lifetime_secs, defaults.max_lifetime_secs);
        assert_eq!(
            connection.connect_timeout_secs,
            defaults.connect_timeout_secs
        );
        assert_eq!(connection.tls_mode, defaults.tls_mode);
        assert!(connection.tls_ca.is_none());
        assert!(connection.tls_client_cert.is_none());
        assert!(connection.tls_client_key.is_none());
        let after =
            ["app.toml", "app.dev.toml"].map(|name| fs::read(fixture.0.join(name)).unwrap());
        assert_eq!(after, before);
    }
}
