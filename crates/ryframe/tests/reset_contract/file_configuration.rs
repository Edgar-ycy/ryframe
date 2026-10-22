use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use ryframe_config::{
    AppConfig, ConfigurationArgs, DbTlsMode, ResetConfig, StorageBackend,
    TenantDatabaseTargetConfig, TenantDatabaseTargetKind, TenantDatabaseTargetMode,
};
use tempfile::TempDir;

use super::*;

const PRIMARY_PASSWORD: &str = "primary-private-marker";
const TENANT_PASSWORD: &str = "tenant-private-marker";
const COMMON: &str = r#"
scope_id = "reset-contract"
snowflake_worker_id = 1
[bootstrap]
admin_password = "Reset!Admin123"
[app]
name = "reset-contract"
host = "127.0.0.1"
port = 19876
[database.primary]
host = "localhost"
port = 3306
database = "reset_control"
username = "reset"
password = "primary-private-marker"
max_connections = 2
min_connections = 0
tls_mode = "required"
[auth]
jwt_secret = "reset-contract-secret-with-thirty-two-bytes"
access_token_expire = "1h"
refresh_token_expire = "24h"
[logger]
level = "info"
format = "text"
output = "stdout"
retention_days = 7
[api_docs]
enabled = false
[redis]
mode = "optional"
host = "127.0.0.1"
port = 6379
password = "redis-private-marker"
database = 0
max_pool_size = 2
timeout_secs = 3
tls = false
[reset]
source_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
credential_version = "rotation-1"
redis_outside_sentinel_key = "outside-reset-sentinel"
"#;

struct Fixture {
    directory: TempDir,
    config: AppConfig,
}

impl Fixture {
    fn new() -> Self {
        fs::create_dir_all(test_state_root()).unwrap();
        let directory = tempfile::tempdir_in(test_state_root()).unwrap();
        let storage = directory.path().join("objects");
        fs::create_dir(&storage).unwrap();
        let source = format!(
            "{COMMON}\n[object_storage]\nbackend = \"local\"\nlocal_base_dir = {}\n",
            serde_json::to_string(storage.to_str().unwrap()).unwrap()
        );
        fs::write(directory.path().join("app.toml"), source).unwrap();
        fs::write(directory.path().join("app.test.toml"), "").unwrap();
        let config = AppConfig::load(directory.path(), Environment::Test).unwrap();
        Self { directory, config }
    }

    fn manifest(&self) -> ResetManifest {
        build_manifest(&self.config, &load_code_sha(&self.config.reset).unwrap()).unwrap()
    }

    fn target(key: &str, database: &str, password: &str) -> TenantDatabaseTargetConfig {
        TenantDatabaseTargetConfig {
            key: key.into(),
            display_name: None,
            region: None,
            mode: TenantDatabaseTargetMode::Dedicated,
            kind: TenantDatabaseTargetKind::Mysql,
            host: Some("localhost".into()),
            port: Some(3306),
            database: Some(database.into()),
            username: Some("reset".into()),
            password: Some(password.into()),
            max_connections: Some(2),
            tls_mode: Some(DbTlsMode::Required),
            tls_ca: None,
            tls_client_cert: None,
            tls_client_key: None,
        }
    }
}

#[test]
fn explicit_directory_and_required_environment_file_are_used_without_process_mutation() {
    let fixture = Fixture::new();
    let args = ConfigurationArgs::parse(vec![
        "plan".into(),
        "--config-dir".into(),
        fixture.directory.path().to_str().unwrap().into(),
    ])
    .unwrap();
    assert_eq!(parse_command(args.arguments).unwrap(), Command::Plan);
    let config = AppConfig::load(args.directory, Environment::Test).unwrap();
    assert_eq!(config.database.primary.password, PRIMARY_PASSWORD);
    assert_eq!(
        config.bootstrap.admin_password.as_deref(),
        Some("Reset!Admin123")
    );
    fs::remove_file(fixture.directory.path().join("app.test.toml")).unwrap();
    assert!(AppConfig::load(fixture.directory.path(), Environment::Test).is_err());
}

#[test]
fn source_sha_is_explicit_validated_normalized_and_never_falls_back() {
    let mut config = ResetConfig::default();
    assert!(
        load_code_sha(&config)
            .unwrap_err()
            .to_string()
            .contains("reset.source_sha")
    );
    for source in [
        "",
        "a",
        "private-source-marker",
        &"g".repeat(40),
        &"a".repeat(41),
    ] {
        config.source_sha = Some(source.into());
        assert!(config.validate(false).is_err());
        let error = load_code_sha(&config).unwrap_err().to_string();
        assert!(!error.contains("private-source-marker"));
        assert!(error.contains("40"));
    }
    config.source_sha = Some("A".repeat(40));
    assert_eq!(load_code_sha(&config).unwrap(), "a".repeat(40));
    config.validate(false).unwrap();
}

#[test]
fn plan_needs_neither_service_confirmation_nor_a_state_directory() {
    let fixture = Fixture::new();
    assert!(!fixture.config.reset.services_stopped);
    assert!(fixture.config.reset.state_directory.is_none());
    validate_database_set(&fixture.manifest()).unwrap();
    assert!(require_services_stopped(&fixture.config.reset).is_err());
    assert!(
        LedgerStore::from_config(&fixture.config.reset, &fixture.manifest(), &"a".repeat(64))
            .is_err()
    );
    let mut reset = fixture.config.reset.clone();
    reset.services_stopped = true;
    require_services_stopped(&reset).unwrap();
    reset.state_directory = Some(fixture.directory.path().join("states"));
    LedgerStore::from_config(&reset, &fixture.manifest(), &"a".repeat(64)).unwrap();
    reset.state_directory = Some(PathBuf::from("relative-state"));
    assert!(LedgerStore::from_config(&reset, &fixture.manifest(), &"a".repeat(64)).is_err());
}

#[test]
fn secrets_are_omitted_and_exact_configuration_references_are_recorded() {
    let mut fixture = Fixture::new();
    fixture.config.tenant_data.targets.push(Fixture::target(
        "tenant-a",
        "reset_tenant",
        TENANT_PASSWORD,
    ));
    fixture.config.object_storage.backend = StorageBackend::Rustfs;
    fixture.config.object_storage.endpoint = "http://127.0.0.1:19000".into();
    fixture.config.object_storage.access_key = "access-private-marker".into();
    fixture.config.object_storage.secret_key = "secret-private-marker".into();
    let manifest = fixture.manifest();
    let text = String::from_utf8(canonical_json(&manifest).unwrap()).unwrap();
    for secret in [
        PRIMARY_PASSWORD,
        TENANT_PASSWORD,
        "Reset!Admin123",
        "redis-private-marker",
        "access-private-marker",
        "secret-private-marker",
    ] {
        assert!(!text.contains(secret));
    }
    let primary = manifest
        .databases
        .iter()
        .find(|item| item.control_baseline)
        .unwrap();
    assert_eq!(
        primary.connection.password_config_key,
        "database.primary.password"
    );
    let tenant = manifest
        .databases
        .iter()
        .find(|item| !item.control_baseline)
        .unwrap();
    assert_eq!(
        tenant.connection.password_config_key,
        "tenant_data.targets.tenant-a.password"
    );
    assert_eq!(
        manifest.redis.unwrap().password_config_key.as_deref(),
        Some("redis.password")
    );
    assert_eq!(
        manifest.object_storage.access_key_config_key.as_deref(),
        Some("object_storage.access_key")
    );
    assert_eq!(
        manifest.object_storage.secret_key_config_key.as_deref(),
        Some("object_storage.secret_key")
    );
}

#[test]
fn primary_reference_wins_for_legal_aliases_and_order_does_not_change_the_manifest() {
    let mut fixture = Fixture::new();
    let primary_db = fixture.config.database.primary.database.clone();
    fixture.config.tenant_data.targets = vec![
        Fixture::target("zz-alias", &primary_db, PRIMARY_PASSWORD),
        Fixture::target("aa-alias", &primary_db, PRIMARY_PASSWORD),
    ];
    let manifest = fixture.manifest();
    assert_eq!(manifest.databases.len(), 1);
    assert_eq!(
        manifest.databases[0].connection.password_config_key,
        "database.primary.password"
    );
    assert_eq!(
        manifest.databases[0].target_keys,
        ["aa-alias", "shared-control", "zz-alias"]
    );
    fixture.config.tenant_data.targets.reverse();
    assert_eq!(fixture.manifest(), manifest);
}

#[test]
fn target_key_order_selects_canonical_reference_instead_of_full_reference_order() {
    let mut fixture = Fixture::new();
    fixture.config.tenant_data.targets = vec![
        Fixture::target("aa-a", "reset_tenant", TENANT_PASSWORD),
        Fixture::target("aa", "reset_tenant", TENANT_PASSWORD),
    ];
    let manifest = fixture.manifest();
    let target = manifest
        .databases
        .iter()
        .find(|item| !item.control_baseline)
        .unwrap();
    assert_eq!(target.target_keys, ["aa", "aa-a"]);
    assert_eq!(
        target.connection.password_config_key,
        "tenant_data.targets.aa.password"
    );
    fixture.config.tenant_data.targets.reverse();
    assert_eq!(fixture.manifest(), manifest);
}

#[test]
fn conflicting_alias_password_username_or_tls_is_rejected_without_echoing_values() {
    let mut fixture = Fixture::new();
    let mut alias = Fixture::target("tenant-alias", "reset_control", PRIMARY_PASSWORD);
    let valid = alias.clone();
    for field in 0..6 {
        alias = valid.clone();
        match field {
            0 => alias.password = Some(TENANT_PASSWORD.into()),
            1 => alias.username = Some("different-user".into()),
            2 => alias.tls_mode = Some(DbTlsMode::Disabled),
            3 => alias.tls_ca = Some("different-ca-reference".into()),
            4 => alias.tls_client_cert = Some("different-cert-reference".into()),
            _ => alias.tls_client_key = Some("different-key-reference".into()),
        }
        fixture.config.tenant_data.targets = vec![alias.clone()];
        let error = build_manifest(&fixture.config, &"a".repeat(40))
            .unwrap_err()
            .to_string();
        assert!(error.contains("凭据或 TLS"));
        assert!(!error.contains(PRIMARY_PASSWORD));
        assert!(!error.contains(TENANT_PASSWORD));
        assert!(!error.contains("different-"));
    }
}

#[test]
fn rotation_version_and_source_are_bound_but_raw_credentials_do_not_enter_plan_hashes() {
    let mut fixture = Fixture::new();
    let manifest = fixture.manifest();
    fixture.config.database.primary.password = "rotated-private-marker".into();
    assert_eq!(fixture.manifest(), manifest);
    fixture.config.reset.credential_version = Some("rotation-2".into());
    assert_ne!(fixture.manifest().config_sha, manifest.config_sha);
    fixture.config.reset.source_sha = Some("b".repeat(40));
    assert_eq!(fixture.manifest().code_sha, "b".repeat(40));
    assert_ne!(
        sha256_hex(&canonical_json(&fixture.manifest()).unwrap()),
        sha256_hex(&canonical_json(&manifest).unwrap())
    );
}

#[test]
fn empty_explicit_database_passwords_and_empty_redis_marker_are_not_environment_lookups() {
    let mut fixture = Fixture::new();
    fixture.config.database.primary.password.clear();
    fixture.config.redis.as_mut().unwrap().password.clear();
    fixture.config.tenant_data.targets = vec![Fixture::target("empty-target", "reset_tenant", "")];
    let manifest = fixture.manifest();
    assert!(manifest.redis.unwrap().password_config_key.is_none());
    assert!(
        manifest
            .databases
            .iter()
            .all(|item| !item.connection.password_config_key.is_empty())
    );
    fixture.config.tenant_data.targets[0].password = None;
    assert!(build_manifest(&fixture.config, &"a".repeat(40)).is_err());
}

#[test]
fn tls_private_key_is_only_a_reference_digest_and_public_material_is_only_a_digest() {
    let mut fixture = Fixture::new();
    let key = fixture.directory.path().join("client.key");
    let certificate = fixture.directory.path().join("client.crt");
    fs::write(&key, "tls-key-private-marker").unwrap();
    fs::write(&certificate, "public-certificate-marker").unwrap();
    fixture.config.database.primary.tls_client_key = Some(key.to_str().unwrap().into());
    fixture.config.database.primary.tls_client_cert = Some(certificate.to_str().unwrap().into());
    let manifest = fixture.manifest();
    let text = String::from_utf8(canonical_json(&manifest).unwrap()).unwrap();
    for hidden in [
        key.to_str().unwrap(),
        "tls-key-private-marker",
        "public-certificate-marker",
    ] {
        assert!(!text.contains(hidden));
    }
    let identity = &manifest.databases[0].connection;
    assert_eq!(
        identity.tls_client_key_ref_sha256.as_deref(),
        Some(sha256_hex(fs::canonicalize(key).unwrap().to_str().unwrap().as_bytes()).as_str())
    );
}

#[test]
fn incompatible_manifest_and_ledger_are_rejected_and_preserved() {
    let fixture = Fixture::new();
    let mut manifest = fixture.manifest();
    manifest.manifest_version = 4;
    assert!(
        validate_manifest_version(&manifest)
            .unwrap_err()
            .to_string()
            .contains("禁止自动转换")
    );
    assert!(
        authorize_execute(
            &manifest,
            &"a".repeat(64),
            &"a".repeat(64),
            &manifest.confirmation_phrase
        )
        .is_err()
    );
    assert!(LedgerStore::new(fixture.directory.path(), &manifest, &"a".repeat(64)).is_err());
    let manifest = fixture.manifest();
    let hash = "a".repeat(64);
    let store = LedgerStore::new(fixture.directory.path(), &manifest, &hash).unwrap();
    let mut old = ResetLedger::new(&manifest, &hash);
    old.ledger_version = 4;
    store.save(&old).unwrap();
    let original = fs::read(store.ledger_path()).unwrap();
    assert!(
        store
            .load_or_create(&manifest, &hash)
            .unwrap_err()
            .to_string()
            .contains("格式不兼容")
    );
    assert_eq!(fs::read(store.ledger_path()).unwrap(), original);
}

#[test]
fn removed_environment_reference_fields_are_rejected_instead_of_silently_ignored() {
    let fixture = Fixture::new();
    let mut value = serde_json::to_value(fixture.manifest()).unwrap();
    let redis = value["redis"].as_object_mut().unwrap();
    redis.remove("password_config_key");
    redis.insert(
        "password_env".into(),
        serde_json::json!("REMOVED_PASSWORD_SOURCE"),
    );
    assert!(serde_json::from_value::<ResetManifest>(value).is_err());
}

#[cfg(unix)]
#[test]
fn linked_state_directory_and_ledger_are_rejected_without_touching_the_target() {
    let fixture = Fixture::new();
    let link = fixture.directory.path().join("linked");
    let target = fixture.directory.path().join("actual");
    fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(LedgerStore::new(&link, &fixture.manifest(), &"a".repeat(64)).is_err());
    let store = LedgerStore::new(&target, &fixture.manifest(), &"a".repeat(64)).unwrap();
    let other = target.join("external.json");
    fs::write(&other, "private-evidence").unwrap();
    std::os::unix::fs::symlink(&other, store.ledger_path()).unwrap();
    assert!(
        store
            .load_or_create(&fixture.manifest(), &"a".repeat(64))
            .is_err()
    );
    assert_eq!(fs::read_to_string(other).unwrap(), "private-evidence");
}

fn directory_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                entries.insert(relative, None);
                visit(root, &path, entries);
            } else {
                assert!(kind.is_file(), "隔离目录中不应出现链接或特殊文件");
                entries.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn reset_cli(fixture: &Fixture, arguments: &[&str], environment: Option<&str>) -> String {
    let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_ryframe-reset"));
    command
        .current_dir(fixture.directory.path())
        .args(arguments);
    command.env_remove("APP_ENV");
    if let Some(environment) = environment {
        command.env("APP_ENV", environment);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().unwrap();
    assert!(!output.status.success(), "无效 CLI 输入必须失败关闭");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn reset_cli_preserves_safe_diagnostics_without_state_writes_or_resource_connections() {
    let fixture = Fixture::new();
    let before = directory_snapshot(fixture.directory.path());
    let argument_sentinel = "reset-cli-argument-private-marker";
    let invalid = format!("--config-dir={argument_sentinel}");
    let diagnostic = reset_cli(&fixture, &["plan", &invalid], None);
    assert!(diagnostic.contains("--config-dir <目录>"));
    assert!(
        !diagnostic.contains("APP_ENV"),
        "配置目录参数必须先于环境读取失败"
    );
    assert!(
        !diagnostic.contains(argument_sentinel),
        "参数错误不得回显输入"
    );
    assert!(
        directory_snapshot(fixture.directory.path()) == before,
        "参数拒绝不得改写隔离目录"
    );

    // 只绑定隔离的被动端口，不启动服务；意外连接也不会接触共享资源。
    let probes = (0..3)
        .map(|_| {
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            probe.set_nonblocking(true).unwrap();
            probe
        })
        .collect::<Vec<_>>();
    let state = fixture.directory.path().join("must-not-create-state");
    let field_sentinel = "reset-cli-field-private-marker";
    let source = COMMON
        .replace("host = \"localhost\"", "host = \"127.0.0.1\"")
        .replace("port = 3306", &format!("port = {}", probes[0].local_addr().unwrap().port()))
        .replace("port = 6379", &format!("port = {}", probes[1].local_addr().unwrap().port()))
        .replace("max_connections = 2", &format!("max_connections = \"{field_sentinel}\""))
        .replace("credential_version = \"rotation-1\"", &format!(
            "credential_version = \"rotation-1\"\nservices_stopped = true\nstate_directory = {}",
            serde_json::to_string(state.to_str().unwrap()).unwrap()
        ));
    let source = format!(
        "{source}\n[object_storage]\nbackend = \"rustfs\"\nendpoint = \"http://127.0.0.1:{}\"\naccess_key = \"access-private-marker\"\nsecret_key = \"secret-private-marker\"\n",
        probes[2].local_addr().unwrap().port()
    );
    fs::write(fixture.directory.path().join("app.toml"), source).unwrap();
    let before = directory_snapshot(fixture.directory.path());
    let diagnostic = reset_cli(
        &fixture,
        &[
            "execute",
            "--config-dir",
            fixture.directory.path().to_str().unwrap(),
            "--plan-hash",
            &"0".repeat(64),
            "--confirm-reset",
            "RESET-RYFRAME-test-reset-contract",
        ],
        Some("test"),
    );
    for detail in [
        "app.toml",
        "app.test.toml",
        "database.primary.max_connections",
        "修正 TOML",
    ] {
        assert!(
            diagnostic.contains(detail),
            "配置诊断必须保留文件、字段与修复提示"
        );
    }
    for secret in [
        field_sentinel,
        PRIMARY_PASSWORD,
        "Reset!Admin123",
        "redis-private-marker",
        "reset-contract-secret-with-thirty-two-bytes",
        "access-private-marker",
        "secret-private-marker",
    ] {
        assert!(
            !diagnostic.contains(secret),
            "配置诊断不得回显配置秘密或无效字段值"
        );
    }
    assert!(!state.exists(), "配置拒绝不得创建状态目录");
    assert!(
        directory_snapshot(fixture.directory.path()) == before,
        "配置拒绝不得改写隔离目录"
    );
    for probe in probes {
        assert!(
            matches!(probe.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "配置拒绝前不得连接 MySQL、Redis 或对象存储"
        );
    }
}
