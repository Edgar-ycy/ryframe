use super::cli::DevOptions;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    dev::{Binaries, DevSession, RuntimeConfig, recovery_seed},
    watch::SourceRevision,
};

#[test]
fn lkg_layout_is_scoped_by_session_and_generation() {
    let fixture = SnapshotFixture::new("layout");
    let (session, recovered) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
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

    let session =
        DevSession::prepare_isolated(&fixture.root, &storage, &DevOptions::default()).unwrap();
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
    assert!(stale_binary.exists());
    assert!(stale_runtime.exists());
    assert!(!fixture.root.join("target/xtask/dev").exists());
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn startup_recovers_the_most_recent_complete_generation_after_crash() {
    let fixture = SnapshotFixture::new("crash-recovery");
    let (session, recovered) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    assert!(recovered.is_none());
    let _older = fixture.install(&session, 1);
    let newest = fixture.install(&session, 2);
    drop(session);

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let recovered = recovered.expect("启动时应恢复最近完整 generation");

    assert_eq!(recovered.generation_dir, newest.generation_dir);
    assert_eq!(recovered.runtime_dir, newest.runtime_dir);
    assert_eq!(recovered.source_revision, SourceRevision::from_value(2));
    assert_eq!(recovery_seed(Some(&recovered)).unwrap().value(), 3);
}

#[test]
fn startup_skips_a_corrupted_newest_generation() {
    let fixture = SnapshotFixture::new("corrupted-newest");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let older = fixture.install(&session, 1);
    let newest = fixture.install(&session, 2);
    fs::write(&newest.worker, b"corrupted worker").unwrap();

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();

    assert_eq!(
        recovered
            .expect("损坏的最新 generation 应回退")
            .generation_dir,
        older.generation_dir
    );
}

#[test]
fn startup_preserves_unowned_copying_and_incomplete_generation_pairs() {
    let fixture = SnapshotFixture::new("incomplete");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
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

    let (_next_session, recovered) =
        DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();

    assert_eq!(
        recovered
            .expect("完整 generation 不应受残缺目录影响")
            .generation_dir,
        complete.generation_dir
    );
    assert!(binary_copying.exists());
    assert!(runtime_copying.exists());
    assert!(incomplete_binary.exists());
    assert!(incomplete_runtime.exists());
    assert!(active_binary_copying.exists());
    assert!(active_runtime_copying.exists());
}

#[test]
fn runtime_config_snapshot_keeps_private_secrets_out_of_public_manifest() {
    let fixture = SnapshotFixture::new("secret-snapshot");
    let secrets = [
        "devex-metrics-secret-1",
        "devex-database-secret-2",
        "devex-jwt-secret-3",
        "devex-redis-secret-4",
        "devex-access-secret-5",
        "devex-storage-secret-6",
    ];
    write_base_config(
        &fixture.root,
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
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let binaries = fixture.install(&session, 1);

    let mut persisted = fs::read(binaries.generation_dir.join("manifest.json")).unwrap();
    persisted.extend(fs::read(&binaries.api).unwrap());
    persisted.extend(fs::read(&binaries.worker).unwrap());
    let persisted = String::from_utf8_lossy(&persisted);
    for secret in secrets {
        assert!(
            !persisted.contains(secret),
            "公开 manifest 与二进制不得持久化敏感值"
        );
    }
    let sanitized = fs::read_to_string(binaries.config_dir.join("app.toml")).unwrap();
    let document = toml::from_str::<toml::Table>(&sanitized).unwrap();
    assert_eq!(
        document["database"]["primary"]["password"].as_str(),
        Some(secrets[1])
    );
    assert_eq!(document["auth"]["jwt_secret"].as_str(), Some(secrets[2]));
    for secret in secrets {
        assert!(sanitized.contains(secret));
    }
}

#[test]
fn database_replica_and_source_passwords_are_kept_in_toml_snapshots() {
    let fixture = SnapshotFixture::new("database-passwords");
    write_base_config(
        &fixture.root,
        "[[database.replicas]]\nname = 'replica'\nhost = 'localhost'\nport = 3306\ndatabase = 'replica'\nusername = 'root'\npassword = 'replica-password'\nmax_connections = 1\nmin_connections = 1\n\n[[database.sources]]\nname = 'source'\nhost = 'localhost'\nport = 3306\ndatabase = 'source'\nusername = 'root'\npassword = 'source-password'\nmax_connections = 1\nmin_connections = 1\n",
    )
    .unwrap();
    fs::create_dir(fixture.root.join(".local-tests")).unwrap();
    let target = fixture.root.join(".local-tests/sanitized-config");
    RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap()
        .snapshot_to(&target)
        .unwrap();
    let sanitized = fs::read_to_string(target.join("app.toml")).unwrap();
    assert!(sanitized.contains("replica-password"));
    assert!(sanitized.contains("source-password"));
}

#[test]
fn non_string_registered_secret_fails_closed_before_snapshot() {
    let fixture = SnapshotFixture::new("non-string-secret");
    write_base_config(&fixture.root, "[database.primary]\npassword = 123456\n").unwrap();

    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("database.primary.password"));
    assert!(error.contains("password"));
    assert!(!error.contains("123456"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn non_string_database_source_password_fails_closed_before_snapshot() {
    let fixture = SnapshotFixture::new("non-string-unregistered-secret");
    write_base_config(
        &fixture.root,
        "[[database.sources]]\nname = 'source'\nhost = 'localhost'\nport = 3306\ndatabase = 'source'\nusername = 'root'\npassword = [123456]\nmax_connections = 1\nmin_connections = 1\n",
    )
    .unwrap();

    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("database.sources"));
    assert!(error.contains("password"));
    assert!(!error.contains("123456"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_rejects_unregistered_non_toml_files() {
    let fixture = SnapshotFixture::new("unknown-non-toml");
    let unknown = fixture.root.join("config/runtime.env");
    fs::write(&unknown, "API_TOKEN=must-not-persist\n").unwrap();
    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("未登记"));
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

    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("未登记"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_rejects_config_subdirectories() {
    let fixture = SnapshotFixture::new("config-subdirectory");
    fs::create_dir(fixture.root.join("config/app.test.toml")).unwrap();

    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("目录") || error.contains("普通"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn snapshot_copies_the_registered_feature_matrix() {
    let fixture = SnapshotFixture::new("feature-matrix");
    let feature_matrix = fixture.root.join("config/feature-matrix.json");
    fs::write(&feature_matrix, "{\"version\":1,\"packages\":[]}\n").unwrap();
    let target = fixture.root.join("snapshot");

    RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap()
        .snapshot_to(&target)
        .unwrap();

    assert_eq!(
        fs::read(target.join("feature-matrix.json")).unwrap(),
        fs::read(feature_matrix).unwrap()
    );
}

#[test]
fn equivalent_config_write_matches_lkg_snapshot_but_semantic_or_secret_change_does_not() {
    let fixture = SnapshotFixture::new("equivalent-config");
    write_base_config(
        &fixture.root,
        "[database.primary]\npassword = 'snapshot-password'\n[app]\nname = 'ryframe'\n",
    )
    .unwrap();
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let binaries = fixture.install(&session, 1);
    let source = fixture.root.join("config");

    assert!(
        RuntimeConfig::capture(&source)
            .unwrap()
            .semantically_matches(&binaries.runtime_config)
            .unwrap()
    );

    fs::write(
        source.join("app.dev.toml"),
        "# 保存但没有语义配置变更\n[app]\n",
    )
    .unwrap();
    assert!(
        RuntimeConfig::capture(&source)
            .unwrap()
            .semantically_matches(&binaries.runtime_config)
            .unwrap()
    );

    fs::write(source.join("app.dev.toml"), "[app]\nname = 'changed'\n").unwrap();
    assert!(
        !RuntimeConfig::capture(&source)
            .unwrap()
            .semantically_matches(&binaries.runtime_config)
            .unwrap()
    );

    write_base_config(
        &fixture.root,
        "[database.primary]\npassword = 'changed-secret'\n[app]\nname = 'ryframe'\n",
    )
    .unwrap();
    fs::write(source.join("app.dev.toml"), "[app]\n").unwrap();
    assert!(
        !RuntimeConfig::capture(&source)
            .unwrap()
            .semantically_matches(&binaries.runtime_config)
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
    symlink(&outside, fixture.root.join("config/app.test.toml")).unwrap();
    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("链接"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[cfg(windows)]
#[test]
fn snapshot_rejects_symbolic_links_when_windows_allows_creation() {
    use std::os::windows::fs::symlink_file;

    let fixture = SnapshotFixture::new("config-symlink");
    let outside = fixture.root.join("outside.toml");
    fs::write(&outside, "[app]\nname = 'outside'\n").unwrap();
    let linked = fixture.root.join("config/app.test.toml");
    if symlink_file(&outside, &linked).is_err() {
        return;
    }
    let error = RuntimeConfig::capture(&fixture.root.join("config"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("链接"));
    assert!(!fixture.root.join(".local-tests/dev-runtime").exists());
}

#[test]
fn recovery_uses_private_password_when_live_source_is_missing() {
    let fixture = SnapshotFixture::new("private-recovery");
    write_base_config(
        &fixture.root,
        "[database.primary]\npassword='generation-only-password'\n",
    )
    .unwrap();
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let saved = fixture.install(&session, 19);
    fs::remove_file(fixture.root.join("config/app.toml")).unwrap();
    fs::remove_file(fixture.root.join("config/app.dev.toml")).unwrap();
    let (_, recovered) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let recovered = recovered.unwrap();
    assert_eq!(recovered.generation_dir, saved.generation_dir);
    assert_eq!(recovered.source_revision.value(), 19);
    assert_eq!(
        recovered.runtime_config.app().database.primary.password,
        "generation-only-password"
    );
}

#[test]
fn public_manifest_has_exact_v3_fields_and_no_secret_registry() {
    let fixture = SnapshotFixture::new("manifest-contract");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let saved = fixture.install(&session, 1);
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(saved.generation_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(document["format_version"], 3);
    assert_eq!(
        document
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "format_version",
            "session",
            "generation",
            "source_revision",
            "created_unix_nanos",
            "api_sha256",
            "worker_sha256",
            "runtime_sha256"
        ]
        .into_iter()
        .collect()
    );
}

// 使用正式配置作为完整基线，场景仅覆盖所测字段，避免手抄 schema 漏项。
pub(super) fn write_base_config(root: &Path, overlay: impl AsRef<str>) -> std::io::Result<()> {
    let mut base: toml::Table = toml::from_str(include_str!("../../../config/app.toml")).unwrap();
    let overlay: toml::Table = toml::from_str(overlay.as_ref()).unwrap();
    merge_fixture_tables(&mut base, overlay);
    fs::write(
        root.join("config/app.toml"),
        toml::to_string(&base).unwrap(),
    )
}

fn merge_fixture_tables(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(current)), toml::Value::Table(next)) => {
                merge_fixture_tables(current, next);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
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
        write_base_config(&root, "").unwrap();
        fs::write(root.join("config/app.dev.toml"), b"[app]\n").unwrap();
        fs::write(root.join("locales/zh-CN.toml"), b"hello = 'world'\n").unwrap();
        let api = root.join("api-source.bin");
        let worker = root.join("worker-source.bin");
        fs::write(&api, b"api").unwrap();
        fs::write(&worker, b"worker").unwrap();
        Self { root, api, worker }
    }

    fn install(&self, session: &DevSession, revision: u64) -> Binaries {
        let runtime = session
            .snapshot_inputs(
                SourceRevision::from_value(revision),
                RuntimeConfig::capture(&self.root.join("config")).unwrap(),
                &self.root.join("locales"),
                None,
            )
            .unwrap();
        session
            .install_generation(
                SourceRevision::from_value(revision),
                &self.api,
                &self.worker,
                runtime,
            )
            .unwrap()
    }
}

impl Drop for SnapshotFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[path = "dev_snapshot/generations.rs"]
mod generations;
