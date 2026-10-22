use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        target_verify::input::{request, request_physical},
    },
};

const UUID: &str = "11111111-1111-1111-1111-111111111111";
const PASSWORD: &str = "fixture-mysql-secret";
const ACCESS: &str = "fixture-object-access";
const SECRET: &str = "fixture-object-secret";

struct Fixture {
    backend: PathBuf,
    defaults: PathBuf,
    target: Value,
    table: Value,
    selected: Value,
    environment: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let backend = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests")
            .join(format!(
                "target-physical-file-{}",
                uuid::Uuid::new_v4().simple()
            ));
        let local = backend.join(".local-tests");
        fs::create_dir_all(backend.parent().unwrap()).unwrap();
        fs::create_dir(&backend).unwrap();
        fs::create_dir(&local).unwrap();
        let defaults = local.join("mysql.cnf");
        fs::write(&defaults, defaults_text()).unwrap();
        let hash = digest::file(&defaults).unwrap().sha256;
        let databases = request::KEYS
            .into_iter()
            .map(|(key, kind, mode)| {
                json!({"key": key, "kind": kind, "mode": mode,
                "database": database_name(key), "server_uuid": UUID,
                "defaults_file": defaults, "defaults_sha256": hash})
            })
            .collect::<Vec<_>>();
        let targets = request::KEYS
            .into_iter()
            .filter(|(key, _, _)| *key != "shared-control")
            .map(|(key, _, mode)| {
                let mut value = connection(key);
                value["key"] = json!(key);
                value["kind"] = json!("mysql");
                value["mode"] = json!(mode);
                value
            })
            .collect::<Vec<_>>();
        let selected = json!({"databases": databases.iter().map(|db| {
            json!({"key": db["key"], "database": db["database"], "expected_server_uuid": UUID,
                "connection_file": defaults, "host": "127.0.0.1", "port": 3306})
        }).collect::<Vec<_>>()});
        let s3 = json!({"endpoint": "http://127.0.0.1:29000", "region": "test-region",
            "access_key": ACCESS, "secret_key": SECRET});
        Self {
            backend,
            defaults,
            target: json!({"scope_id": "file-scope", "databases": databases, "s3": s3}),
            table: json!({"scope_id": "file-scope", "database": {"primary": connection("shared-control")},
                "tenant_data": {"targets": targets}, "object_storage": {
                    "backend": "rustfs", "endpoint": "http://127.0.0.1:29000", "region": "test-region",
                    "access_key": ACCESS, "secret_key": SECRET, "use_ssl": false}}),
            selected,
            environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        }
    }

    fn binding(&self) -> Result<Value> {
        request_physical::binding(
            &self.backend,
            &self.target,
            &self.table,
            &self.environment,
            &self.selected,
        )
    }

    fn reject(&self) -> String {
        let message = self.binding().unwrap_err().to_string();
        for secret in [PASSWORD, ACCESS, SECRET, "changed-private-value"] {
            assert!(!message.contains(secret));
        }
        message
    }

    fn connection_mut(&mut self, control: bool) -> &mut Value {
        if control {
            &mut self.table["database"]["primary"]
        } else {
            &mut self.table["tenant_data"]["targets"][0]
        }
    }

    fn defaults(&mut self, raw: &str) {
        fs::write(&self.defaults, raw).unwrap();
        let hash = digest::file(&self.defaults).unwrap().sha256;
        for database in self.target["databases"].as_array_mut().unwrap() {
            database["defaults_sha256"] = json!(hash);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.backend).unwrap();
    }
}

fn database_name(key: &str) -> String {
    format!("fixture_{}", key.replace('-', "_"))
}

fn connection(key: &str) -> Value {
    json!({"host": "127.0.0.1", "port": 3306, "database": database_name(key),
        "username": "fixture", "password": PASSWORD, "tls_mode": "disabled"})
}

fn defaults_text() -> String {
    format!(
        "[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword={PASSWORD}\nssl-mode=DISABLED\n"
    )
}

#[test]
fn file_table_binds_complete_databases_and_returns_only_safe_physical_fields() {
    let fixture = Fixture::new();
    let mut result = fixture.binding().unwrap();
    let hash = result.as_object_mut().unwrap().remove("sha256").unwrap();
    assert_eq!(hash, json!(digest::plan_hash(&result).unwrap()));
    assert_eq!(result["scope_id"], fixture.table["scope_id"]);
    let databases = result["databases"].as_array().unwrap();
    assert_eq!(databases.len(), 4);
    let mut keys = Vec::new();
    for db in databases {
        let key = db["key"].as_str().unwrap();
        keys.push(key);
        let (_, kind, mode) = request::KEYS
            .iter()
            .find(|(expected, _, _)| *expected == key)
            .unwrap();
        assert_eq!(db["kind"], *kind);
        assert_eq!(db["mode"], *mode);
        assert_eq!(db["server_uuid"], UUID);
        assert_eq!(db["database"], database_name(key));
        assert_eq!(db["host"], "127.0.0.1");
        assert_eq!(db["port"], 3306);
        assert_eq!(db["tls_mode"], "disabled");
    }
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        result["s3"],
        json!({"backend": "rustfs", "endpoint": "http://127.0.0.1:29000", "region": "test-region"})
    );
    let public = result.to_string();
    for private in [
        PASSWORD,
        ACCESS,
        SECRET,
        "username",
        "password",
        "access_key",
        "secret_key",
        "defaults_file",
    ] {
        assert!(!public.contains(private));
    }
}

#[test]
fn scope_must_come_from_the_table_and_match_the_target() {
    for scope in [
        Value::Null,
        json!(""),
        json!("wrong-scope"),
        json!(" file-scope"),
    ] {
        let mut fixture = Fixture::new();
        fixture
            .environment
            .insert("APP_SCOPE_ID".into(), "file-scope".into());
        fixture.table["scope_id"] = scope;
        fixture.reject();
    }
    let mut fixture = Fixture::new();
    fixture.target["scope_id"] = json!("wrong-scope");
    fixture.reject();
}

#[test]
fn app_env_must_be_exactly_test_without_reading_other_environment_fields() {
    for value in [
        None,
        Some(""),
        Some("dev"),
        Some("prod"),
        Some("TEST"),
        Some("test\n"),
    ] {
        let mut fixture = Fixture::new();
        fixture.environment.clear();
        if let Some(value) = value {
            fixture.environment.insert("APP_ENV".into(), value.into());
        }
        fixture.reject();
    }
    Fixture::new().binding().unwrap();
}

#[test]
fn legacy_environment_overrides_and_file_indirection_cannot_change_binding() {
    let mut fixture = Fixture::new();
    let expected = fixture.binding().unwrap();
    for name in [
        "APP_SCOPE_ID",
        "APP_DATABASE_HOST",
        "APP_DATABASE_PORT",
        "APP_DATABASE_NAME",
        "APP_DATABASE_USERNAME",
        "APP_DATABASE_PASSWORD",
        "APP_DATABASE_TLS_MODE",
        "APP_DATABASE_TLS_CA",
        "APP_DATABASE_TLS_CLIENT_CERT",
        "APP_DATABASE_TLS_CLIENT_KEY",
        "APP_DATABASE_REPLICAS",
        "APP_DATABASE_SOURCES",
        "APP_TENANT_DATA_TARGETS",
        "APP_OBJECT_STORAGE_BACKEND",
        "APP_OBJECT_STORAGE_ENDPOINT",
        "APP_OBJECT_STORAGE_REGION",
        "APP_OBJECT_STORAGE_ACCESS_KEY",
        "APP_OBJECT_STORAGE_SECRET_KEY",
        "APP_OBJECT_STORAGE_USE_SSL",
        "APP_TENANT_PASSWORD",
    ] {
        fixture
            .environment
            .insert(name.into(), "changed-private-value".into());
        fixture
            .environment
            .insert(format!("{name}_FILE"), "changed-private-value".into());
    }
    assert_eq!(fixture.binding().unwrap(), expected);
}

#[test]
fn database_password_must_be_explicit_nonempty_text_and_match_defaults() {
    for control in [false, true] {
        for value in [
            Value::Null,
            json!(""),
            json!(" "),
            json!(42),
            json!("changed-private-value"),
        ] {
            let mut fixture = Fixture::new();
            fixture.connection_mut(control)["password"] = value;
            fixture.reject();
        }
        let mut fixture = Fixture::new();
        fixture
            .connection_mut(control)
            .as_object_mut()
            .unwrap()
            .remove("password");
        fixture.reject();
    }
}

#[test]
fn legacy_database_password_env_is_rejected_even_with_a_plain_password() {
    for control in [false, true] {
        for value in [Value::Null, json!("APP_TENANT_PASSWORD")] {
            let mut fixture = Fixture::new();
            fixture.connection_mut(control)["password_env"] = value;
            fixture
                .environment
                .insert("APP_TENANT_PASSWORD".into(), PASSWORD.into());
            assert!(fixture.reject().contains("password"));
        }
    }
}

#[test]
fn object_credentials_are_plain_matching_text_on_both_sides() {
    for planned in [false, true] {
        for field in ["access_key", "secret_key"] {
            for value in [
                Value::Null,
                json!(""),
                json!(" "),
                json!(42),
                json!("changed-private-value"),
            ] {
                let mut fixture = Fixture::new();
                if planned {
                    fixture.target["s3"][field] = value;
                } else {
                    fixture.table["object_storage"][field] = value;
                }
                fixture.reject();
            }
        }
    }
}

#[test]
fn object_legacy_env_keys_are_rejected_even_when_plain_credentials_match() {
    for planned in [false, true] {
        for field in ["access_key", "secret_key"] {
            for value in [Value::Null, json!("APP_OBJECT_STORAGE_SECRET_KEY")] {
                let mut fixture = Fixture::new();
                if planned {
                    fixture.target["s3"][field] = value;
                } else {
                    fixture.table["object_storage"][field] = value;
                }
                fixture.reject();
            }
        }
    }
}

#[test]
fn same_physical_database_is_rejected_including_case_only_name_changes() {
    for name in [
        database_name("shared-control"),
        database_name("shared-control").to_uppercase(),
    ] {
        let mut fixture = Fixture::new();
        fixture.target["databases"][1]["database"] = json!(name);
        fixture.table["tenant_data"]["targets"][0]["database"] = json!(name);
        assert!(fixture.reject().contains("同一物理数据库"));
    }
}

#[test]
fn replicas_sources_and_non_array_collections_cannot_be_masked_by_environment() {
    for field in ["replicas", "sources"] {
        for value in [json!([{"host": "127.0.0.1"}]), json!({}), Value::Null] {
            let mut fixture = Fixture::new();
            fixture.table["database"][field] = value;
            fixture.environment.insert(
                format!("APP_DATABASE_{}", field.to_uppercase()),
                "[]".into(),
            );
            fixture.reject();
        }
        let mut fixture = Fixture::new();
        fixture.table["database"][field] = json!([]);
        fixture.binding().unwrap();
    }
    let mut fixture = Fixture::new();
    fixture.table["tenant_data"]["targets"] = json!({});
    fixture.reject();
}

#[test]
fn implicit_and_explicit_control_targets_produce_the_same_binding() {
    let mut fixture = Fixture::new();
    let expected = fixture.binding().unwrap();
    fixture.table["tenant_data"]["targets"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key": "shared-control", "kind": "control", "mode": "shared"}));
    assert_eq!(fixture.binding().unwrap(), expected);
}

#[test]
fn control_target_cannot_redeclare_a_connection_or_legacy_password_key() {
    for field in [
        "host",
        "port",
        "database",
        "username",
        "password",
        "tls_mode",
        "tls_ca",
        "tls_client_cert",
        "tls_client_key",
        "max_connections",
        "password_env",
    ] {
        let mut fixture = Fixture::new();
        let mut control = json!({"key": "shared-control", "kind": "control", "mode": "shared"});
        control[field] = json!("changed-private-value");
        fixture.table["tenant_data"]["targets"]
            .as_array_mut()
            .unwrap()
            .push(control);
        fixture.reject();
    }
    let mut fixture = Fixture::new();
    fixture.table["tenant_data"]["targets"].as_array_mut().unwrap()
        .push(json!({"key": "shared-control", "kind": "control", "mode": "shared", "password_env": null}));
    fixture.reject();
}

#[test]
fn key_sets_and_duplicate_keys_must_exactly_cover_the_declared_targets() {
    let mut fixture = Fixture::new();
    fixture.table["tenant_data"]["targets"][0]["key"] = json!("unexpected-key");
    fixture.reject();
    let mut fixture = Fixture::new();
    let duplicate = fixture.table["tenant_data"]["targets"][0].clone();
    fixture.table["tenant_data"]["targets"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    fixture.reject();
    let mut fixture = Fixture::new();
    fixture.target["databases"].as_array_mut().unwrap().pop();
    fixture.reject();
}

#[test]
fn target_kind_mode_and_reviewed_addresses_remain_required() {
    for (field, value) in [("kind", json!("control")), ("mode", json!("dedicated"))] {
        let mut fixture = Fixture::new();
        fixture.table["tenant_data"]["targets"][0][field] = value;
        fixture.reject();
    }
    for field in ["host", "port"] {
        let mut fixture = Fixture::new();
        fixture.selected["databases"][0][field] = json!("changed-private-value");
        fixture.reject();
    }
    let mut fixture = Fixture::new();
    fixture.selected["databases"] = json!([]);
    fixture.reject();
}

#[test]
fn external_tls_files_and_mismatched_tls_are_rejected() {
    for control in [false, true] {
        for field in ["tls_ca", "tls_client_cert", "tls_client_key"] {
            let mut fixture = Fixture::new();
            fixture.connection_mut(control)[field] = json!("changed-private-value");
            fixture.reject();
        }
        let mut fixture = Fixture::new();
        fixture.connection_mut(control)["tls_mode"] = json!("required");
        fixture.reject();
    }
}

#[test]
fn required_tls_is_preserved_in_every_physical_database_description() {
    let mut fixture = Fixture::new();
    fixture.defaults(&defaults_text().replace("ssl-mode=DISABLED", "ssl-mode=REQUIRED"));
    fixture.table["database"]["primary"]["tls_mode"] = json!("required");
    for target in fixture.table["tenant_data"]["targets"]
        .as_array_mut()
        .unwrap()
    {
        target["tls_mode"] = json!("required");
    }
    let result = fixture.binding().unwrap();
    assert!(
        result["databases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|database| database["tls_mode"] == "required")
    );
}

#[test]
fn defaults_require_the_exact_sha_and_local_test_file_boundary() {
    let mut fixture = Fixture::new();
    fixture.target["databases"][0]["defaults_sha256"] = json!("0".repeat(64));
    fixture.reject();
    let mut fixture = Fixture::new();
    let outside = fixture.backend.join("outside.cnf");
    fs::write(&outside, defaults_text()).unwrap();
    fixture.target["databases"][0]["defaults_file"] = json!(outside);
    fixture.reject();
    let mut fixture = Fixture::new();
    fixture.target["databases"][0]["defaults_file"] = json!(fixture.backend.join(".local-tests"));
    fixture.reject();
}

#[test]
fn defaults_keep_minimal_five_option_parsing_loopback_and_tls_rules() {
    let valid = defaults_text();
    for raw in [
        valid.replace("[client]", "[other]"),
        valid.replace("host=127.0.0.1\n", ""),
        format!("{valid}database=unexpected\n"),
        format!("{valid}host=127.0.0.1\n"),
        valid.replace("password=fixture-mysql-secret", "password="),
        valid.replace("password=fixture-mysql-secret", "password=\"bad#token\""),
        valid.replace("host=127.0.0.1", "host=example.invalid"),
        valid.replace("port=3306", "port=0"),
        valid.replace("port=3306", "port=65536"),
        valid.replace("ssl-mode=DISABLED", "ssl-mode=VERIFY_IDENTITY"),
    ] {
        let mut fixture = Fixture::new();
        fixture.defaults(&raw);
        fixture.reject();
    }
    let mut fixture = Fixture::new();
    fixture.defaults(&valid.replace(PASSWORD, &format!("\"{PASSWORD}\"")));
    fixture.binding().unwrap();
}

#[test]
fn object_endpoint_region_and_explicit_ssl_rules_remain_unchanged() {
    for endpoint in [
        "http://localhost:29000",
        "http://example.invalid:29000",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://127.0.0.1:29000/path",
        "http://127.0.0.1:29000/?x=1",
    ] {
        let mut fixture = Fixture::new();
        fixture.table["object_storage"]["endpoint"] = json!(endpoint);
        fixture.target["s3"]["endpoint"] = json!(endpoint);
        fixture.reject();
    }
    for (field, value) in [
        ("region", json!("other-region")),
        ("use_ssl", json!(true)),
        ("use_ssl", json!("false")),
        ("backend", json!("local")),
    ] {
        let mut fixture = Fixture::new();
        fixture.table["object_storage"][field] = value;
        fixture.reject();
    }
    let mut fixture = Fixture::new();
    fixture.table["object_storage"]["endpoint"] = json!("https://[::1]:29000/");
    fixture.table["object_storage"]["use_ssl"] = json!(true);
    fixture.target["s3"]["endpoint"] = json!("https://[::1]:29000");
    assert_eq!(
        fixture.binding().unwrap()["s3"]["endpoint"],
        "https://[::1]:29000"
    );
}
