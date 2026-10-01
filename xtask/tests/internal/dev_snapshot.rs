use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    dev::{
        Binaries, DevSession, RuntimeSecrets, runtime_config_matches_snapshot, snapshot_config_tree,
    },
    watch::SourceRevision,
};

#[test]
fn lkg_layout_is_scoped_by_session_and_generation() {
    let fixture = SnapshotFixture::new("layout");
    let (session, recovered) =
        DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    assert!(recovered.is_none());

    let binaries = fixture.install(&session, 1);
    let binary_session = binaries.generation_dir.parent().unwrap();
    let runtime_session = binaries.runtime_dir.parent().unwrap();
    assert_eq!(
        binary_session.parent(),
        Some(fixture.root.join("target/xtask/dev").as_path())
    );
    assert_eq!(
        runtime_session.parent(),
        Some(fixture.root.join(".local-tests/dev-runtime").as_path())
    );
    assert_eq!(binary_session.file_name(), runtime_session.file_name());
    assert_eq!(
        binaries.generation_dir.file_name(),
        binaries.runtime_dir.file_name()
    );
    assert!(
        binary_session
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("s-"))
    );
    assert!(
        binaries
            .generation_dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("g-"))
    );
    assert!(binaries.generation_dir.join("manifest.json").is_file());
    assert!(binaries.api.is_file());
    assert!(binaries.worker.is_file());
    assert!(binaries.config_dir.is_dir());
    assert!(binaries.locales_dir.is_dir());
}

#[test]
fn measurement_session_uses_an_isolated_non_recoverable_root() {
    let fixture = SnapshotFixture::new("isolated");
    let storage = fixture.root.join("measurement-state");
    let stale_binary = storage.join("binaries/s-4294967295-1/g-1-1");
    let stale_runtime = storage.join("runtime/s-4294967295-1/g-1-1");
    fs::create_dir_all(&stale_binary).unwrap();
    fs::create_dir_all(&stale_runtime).unwrap();

    let session = DevSession::prepare_isolated(&storage, SourceRevision::from_value(0)).unwrap();
    let binaries = fixture.install(&session, 1);

    assert_eq!(
        binaries
            .generation_dir
            .parent()
            .and_then(|path| path.parent()),
        Some(storage.join("binaries").as_path())
    );
    assert_eq!(
        binaries.runtime_dir.parent().and_then(|path| path.parent()),
        Some(storage.join("runtime").as_path())
    );
    assert!(!stale_binary.exists());
    assert!(!stale_runtime.exists());
    assert!(!fixture.root.join("target/xtask/dev").exists());
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn startup_recovers_the_most_recent_complete_generation_after_crash() {
    let fixture = SnapshotFixture::new("crash-recovery");
    let (session, recovered) =
        DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    assert!(recovered.is_none());
    let _older = fixture.install(&session, 1);
    let newest = fixture.install(&session, 2);
    drop(session);

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, SourceRevision::from_value(42)).unwrap();
    let recovered = recovered.expect("启动时应恢复最近完整 generation");

    assert_eq!(recovered.generation_dir, newest.generation_dir);
    assert_eq!(recovered.runtime_dir, newest.runtime_dir);
    assert_eq!(recovered.source_revision, SourceRevision::from_value(42));
}

#[test]
fn startup_skips_a_corrupted_newest_generation() {
    let fixture = SnapshotFixture::new("corrupted-newest");
    let (session, _) = DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    let older = fixture.install(&session, 1);
    let newest = fixture.install(&session, 2);
    fs::write(&newest.worker, b"corrupted worker").unwrap();

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, SourceRevision::from_value(9)).unwrap();

    assert_eq!(
        recovered
            .expect("损坏的最新 generation 应回退")
            .generation_dir,
        older.generation_dir
    );
}

#[test]
fn startup_removes_stale_copying_and_incomplete_generation_pairs() {
    let fixture = SnapshotFixture::new("incomplete");
    let (session, _) = DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    let complete = fixture.install(&session, 1);
    let active_binary_copying = complete
        .generation_dir
        .parent()
        .unwrap()
        .join(".g-8-1.copying");
    let active_runtime_copying = complete
        .runtime_dir
        .parent()
        .unwrap()
        .join(".g-8-1.copying");
    fs::create_dir_all(&active_binary_copying).unwrap();
    fs::create_dir_all(&active_runtime_copying).unwrap();
    let stale_session = "s-4294967295-1";
    let binary_session = fixture.root.join("target/xtask/dev").join(stale_session);
    let runtime_session = fixture
        .root
        .join(".local-tests/dev-runtime")
        .join(stale_session);
    let binary_copying = binary_session.join(".g-9-1.copying");
    let runtime_copying = runtime_session.join(".g-9-1.copying");
    let incomplete_binary = binary_session.join("g-10-2");
    let incomplete_runtime = runtime_session.join("g-10-2");
    fs::create_dir_all(&binary_copying).unwrap();
    fs::create_dir_all(&runtime_copying).unwrap();
    fs::create_dir_all(incomplete_binary.join("bin")).unwrap();
    fs::create_dir_all(incomplete_runtime.join("config")).unwrap();
    fs::write(incomplete_binary.join("manifest.json"), b"{}").unwrap();

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, SourceRevision::from_value(3)).unwrap();

    assert_eq!(
        recovered
            .expect("完整 generation 不应受残缺目录影响")
            .generation_dir,
        complete.generation_dir
    );
    assert!(!binary_copying.exists());
    assert!(!runtime_copying.exists());
    assert!(!incomplete_binary.exists());
    assert!(!incomplete_runtime.exists());
    assert!(active_binary_copying.exists());
    assert!(active_runtime_copying.exists());
}

#[test]
fn runtime_config_snapshot_never_persists_registered_secret_values() {
    let fixture = SnapshotFixture::new("secret-snapshot");
    let secrets = [
        "devex-metrics-secret-1",
        "devex-database-secret-2",
        "devex-jwt-secret-3",
        "devex-redis-secret-4",
        "devex-access-secret-5",
        "devex-storage-secret-6",
    ];
    fs::write(
        fixture.root.join("config/app.toml"),
        format!(
            "[monitor]\nmetrics_bearer_token = '{}'\n\
             [database.primary]\npassword = '{}'\n\
             [auth]\njwt_secret = '{}'\n\
             [redis]\npassword = '{}'\n\
             [object_storage]\naccess_key = '{}'\nsecret_key = '{}'\n",
            secrets[0], secrets[1], secrets[2], secrets[3], secrets[4], secrets[5]
        ),
    )
    .unwrap();
    let (session, _) = DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    let binaries = fixture.install(&session, 1);

    let mut persisted = fs::read(binaries.generation_dir.join("manifest.json")).unwrap();
    persisted.extend(fs::read(&binaries.api).unwrap());
    persisted.extend(fs::read(&binaries.worker).unwrap());
    for entry in fs::read_dir(&binaries.config_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            persisted.extend(fs::read(entry.path()).unwrap());
        }
    }
    let persisted = String::from_utf8_lossy(&persisted);
    for secret in secrets {
        assert!(
            !persisted.contains(secret),
            "运行快照、manifest 与二进制不得持久化敏感值"
        );
    }
    let sanitized = fs::read_to_string(binaries.config_dir.join("app.toml")).unwrap();
    let document = toml::from_str::<toml::Table>(&sanitized).unwrap();
    assert_eq!(
        document["database"]["primary"]["password"].as_str(),
        Some("")
    );
    assert_eq!(document["auth"]["jwt_secret"].as_str(), Some(""));
}

#[test]
fn database_replica_and_source_passwords_are_kept_in_toml_snapshots() {
    let fixture = SnapshotFixture::new("database-passwords");
    fs::write(
        fixture.root.join("config/app.toml"),
        "[[database.replicas]]\nname = 'replica'\nhost = 'localhost'\nport = 3306\ndatabase = 'replica'\nusername = 'root'\npassword = 'replica-password'\nmax_connections = 1\nmin_connections = 1\n\n[[database.sources]]\nname = 'source'\nhost = 'localhost'\nport = 3306\ndatabase = 'source'\nusername = 'root'\npassword = 'source-password'\nmax_connections = 1\nmin_connections = 1\n",
    )
    .unwrap();
    let target = fixture.root.join(".local-tests/sanitized-config");
    snapshot_config_tree(&fixture.root.join("config"), &target).unwrap();
    let sanitized = fs::read_to_string(target.join("app.toml")).unwrap();
    assert!(sanitized.contains("replica-password"));
    assert!(sanitized.contains("source-password"));
}

#[test]
fn non_string_registered_secret_fails_closed_before_snapshot() {
    let fixture = SnapshotFixture::new("non-string-secret");
    fs::write(
        fixture.root.join("config/app.toml"),
        "[database.primary]\npassword = 123456\n",
    )
    .unwrap();

    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("database.primary.password"));
    assert!(error.contains("值不是字符串"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn non_string_database_source_password_fails_closed_before_snapshot() {
    let fixture = SnapshotFixture::new("non-string-unregistered-secret");
    fs::write(
        fixture.root.join("config/app.toml"),
        "[[database.sources]]\nname = 'source'\npassword = [123456]\n",
    )
    .unwrap();

    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("database.sources.[].password"));
    assert!(error.contains("数据库密码"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_rejects_unregistered_non_toml_files() {
    let fixture = SnapshotFixture::new("unknown-non-toml");
    let unknown = fixture.root.join("config/runtime.env");
    fs::write(&unknown, "API_TOKEN=must-not-persist\n").unwrap();
    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("拒绝未登记的配置文件"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_rejects_unregistered_toml_files() {
    let fixture = SnapshotFixture::new("unknown-toml");
    fs::write(
        fixture.root.join("config/secrets.toml"),
        "api_token = 'must-not-persist'\n",
    )
    .unwrap();

    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("拒绝未登记的配置文件"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_rejects_config_subdirectories() {
    let fixture = SnapshotFixture::new("config-subdirectory");
    fs::create_dir(fixture.root.join("config/nested")).unwrap();

    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("拒绝目录或特殊文件"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_copies_the_registered_feature_matrix() {
    let fixture = SnapshotFixture::new("feature-matrix");
    let feature_matrix = fixture.root.join("config/feature-matrix.json");
    fs::write(&feature_matrix, "{\"version\":1,\"packages\":[]}\n").unwrap();
    let target = fixture.root.join("snapshot");

    snapshot_config_tree(&fixture.root.join("config"), &target).unwrap();

    assert_eq!(
        fs::read(target.join("feature-matrix.json")).unwrap(),
        fs::read(feature_matrix).unwrap()
    );
}

#[test]
fn equivalent_config_write_matches_lkg_snapshot_but_semantic_or_secret_change_does_not() {
    let fixture = SnapshotFixture::new("equivalent-config");
    fs::write(
        fixture.root.join("config/app.toml"),
        "[database.primary]\npassword = 'snapshot-password'\n[app]\nname = 'ryframe'\n",
    )
    .unwrap();
    let (session, _) = DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    let binaries = fixture.install(&session, 1);
    let source = fixture.root.join("config");

    assert!(
        runtime_config_matches_snapshot(
            &source,
            &binaries.config_dir,
            binaries.runtime_secrets.as_ref(),
        )
        .unwrap()
    );

    fs::write(
        source.join("app.dev.toml"),
        "# 保存但没有语义配置变更\n[app]\n",
    )
    .unwrap();
    assert!(
        runtime_config_matches_snapshot(
            &source,
            &binaries.config_dir,
            binaries.runtime_secrets.as_ref(),
        )
        .unwrap()
    );

    fs::write(source.join("app.dev.toml"), "[app]\nname = 'changed'\n").unwrap();
    assert!(
        !runtime_config_matches_snapshot(
            &source,
            &binaries.config_dir,
            binaries.runtime_secrets.as_ref(),
        )
        .unwrap()
    );

    fs::write(
        source.join("app.toml"),
        "[database.primary]\npassword = 'changed-secret'\n[app]\nname = 'ryframe'\n",
    )
    .unwrap();
    fs::write(source.join("app.dev.toml"), "[app]\n").unwrap();
    assert!(
        !runtime_config_matches_snapshot(
            &source,
            &binaries.config_dir,
            binaries.runtime_secrets.as_ref(),
        )
        .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn snapshot_rejects_symbolic_links() {
    use std::os::unix::fs::symlink;

    let fixture = SnapshotFixture::new("config-symlink");
    let outside = fixture.root.join("outside.toml");
    fs::write(&outside, "[app]\nname = 'outside'\n").unwrap();
    symlink(&outside, fixture.root.join("config/linked.toml")).unwrap();
    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("拒绝符号链接"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[cfg(windows)]
#[test]
fn snapshot_rejects_symbolic_links_when_windows_allows_creation() {
    use std::os::windows::fs::symlink_file;

    let fixture = SnapshotFixture::new("config-symlink");
    let outside = fixture.root.join("outside.toml");
    fs::write(&outside, "[app]\nname = 'outside'\n").unwrap();
    let linked = fixture.root.join("config/linked.toml");
    if symlink_file(&outside, &linked).is_err() {
        return;
    }
    let error = DevSession::prepare(&fixture.root, SourceRevision::from_value(0))
        .unwrap_err()
        .to_string();

    assert!(error.contains("拒绝符号链接"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn recovery_fails_closed_when_required_secret_environment_is_missing() {
    let fixture = SnapshotFixture::new("missing-recovery-secret");
    fs::write(
        fixture.root.join("config/app.toml"),
        "[database.primary]\npassword = 'generation-only-password'\n",
    )
    .unwrap();
    let (session, _) = DevSession::prepare(&fixture.root, SourceRevision::from_value(0)).unwrap();
    let _binaries = fixture.install(&session, 1);
    drop(session);

    let error = DevSession::prepare_with_runtime_secrets(
        &fixture.root,
        SourceRevision::from_value(2),
        &RuntimeSecrets::default(),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("恢复 last-known-good 缺少当前进程密钥环境"));
    assert!(error.contains("APP_DATABASE_PASSWORD"));
    assert!(!error.contains("generation-only-password"));
}

#[test]
fn runtime_secret_registry_matches_config_secret_override_fact_source() {
    let spec = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/ryframe-config/src/app_config/environment_overrides/spec.rs"),
    )
    .unwrap();
    let compact = spec.split_whitespace().collect::<String>();
    let registered = RuntimeSecrets::registered_environment();

    assert_eq!(
        compact.matches("EnvOverride::secret(").count(),
        registered.len(),
        "新增 config secret override 时必须同步 runtime snapshot 注册表"
    );
    for environment in registered {
        assert!(
            compact.contains(&format!("EnvOverride::secret(\"{environment}\"")),
            "runtime snapshot 缺少 config secret override {environment}"
        );
    }
}

struct SnapshotFixture {
    root: PathBuf,
    api: PathBuf,
    worker: PathBuf,
}

impl SnapshotFixture {
    fn new(label: &str) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ryframe-dev-snapshot-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("locales")).unwrap();
        fs::write(root.join("config/app.dev.toml"), b"[app]\n").unwrap();
        fs::write(root.join("locales/zh-CN.toml"), b"hello = 'world'\n").unwrap();
        let api = root.join("api-source.bin");
        let worker = root.join("worker-source.bin");
        fs::write(&api, b"api").unwrap();
        fs::write(&worker, b"worker").unwrap();
        Self { root, api, worker }
    }

    fn install(&self, session: &DevSession, revision: u64) -> Binaries {
        session
            .install_generation(
                SourceRevision::from_value(revision),
                &self.api,
                &self.worker,
                &self.root.join("config"),
                &self.root.join("locales"),
            )
            .unwrap()
    }
}

impl Drop for SnapshotFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
