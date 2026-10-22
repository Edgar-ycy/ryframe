use super::*;
use crate::{
    cli::{self, Command, DevOptions},
    dev::{SaveCase, switch_services_with_rollback},
    watch::{SourceWatcher, WatchInputRoots},
};
use ryframe_config::{AppConfig, Environment};

fn app(directory: &std::path::Path) -> AppConfig {
    AppConfig::load(directory, Environment::Dev).unwrap()
}

fn configure(fixture: &SnapshotFixture, api: u16, worker: u16, id: u16, password: &str) {
    fs::write(fixture.root.join("config/app.dev.toml"),format!("snowflake_worker_id={id}\n[app]\nport={api}\n[jobs]\nhealth_port={worker}\n[database.primary]\npassword='{password}'\n")).unwrap();
}

#[test]
fn saved_ports_and_ids_switch_without_rebuilding_and_old_generation_is_immutable() {
    let fixture = SnapshotFixture::new("ports-identifiers");
    configure(&fixture, 18080, 19091, 1023, "old-password");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let old = fixture.install(&session, 41);
    let old_api = fs::read(old.runtime_dir.join("api/app.dev.toml")).unwrap();
    configure(&fixture, 28080, 29091, 0, "new-password");
    let captured = RuntimeConfig::capture(&fixture.root.join("config")).unwrap();
    let runtime = session
        .snapshot_inputs(
            SourceRevision::from_value(42),
            captured,
            &fixture.root.join("locales"),
            Some(&old),
        )
        .unwrap();
    // 完全复用旧二进制，配置代次切换无需构建。
    let next = session
        .install_generation(
            SourceRevision::from_value(42),
            &old.api,
            &old.worker,
            runtime,
        )
        .unwrap();
    assert_eq!(fs::read(&next.api).unwrap(), fs::read(&old.api).unwrap());
    let api = app(&next.runtime_dir.join("api"));
    let worker = app(&next.runtime_dir.join("worker"));
    assert_eq!((api.app.port, worker.jobs.health_port), (28080, 29091));
    assert_eq!(
        (api.snowflake_worker_id, worker.snowflake_worker_id),
        (0, 1)
    );
    assert_eq!(worker.jobs.worker_id.as_deref(), Some("ryframe-dev-worker"));
    let probe_api = app(&next.runtime_dir.join("probe-api"));
    let probe_worker = app(&next.runtime_dir.join("probe-worker"));
    let excluded = [1023, 0, 1];
    assert!(!excluded.contains(&probe_api.snowflake_worker_id));
    assert!(!excluded.contains(&probe_worker.snowflake_worker_id));
    assert_ne!(
        probe_api.snowflake_worker_id,
        probe_worker.snowflake_worker_id
    );
    assert_eq!(
        probe_worker.jobs.worker_id.as_deref(),
        Some("ryframe-dev-probe")
    );
    assert_eq!(probe_api.app.host, "127.0.0.1");
    assert_eq!(probe_worker.jobs.health_host, "127.0.0.1");
    assert!(!probe_api.telemetry.enabled);
    assert_eq!(
        fs::read(old.runtime_dir.join("api/app.dev.toml")).unwrap(),
        old_api
    );
    assert_eq!(
        app(&old.runtime_dir.join("api")).database.primary.password,
        "old-password"
    );
    for role in [
        "source-config",
        "locales",
        "api",
        "worker",
        "probe-api",
        "probe-worker",
        "migrate",
    ] {
        assert!(next.runtime_dir.join(role).is_dir());
    }
}

#[test]
fn failed_activation_restores_previous_ports_ids_and_password_from_role_files() {
    let fixture = SnapshotFixture::new("restore-role-config");
    configure(&fixture, 18081, 19092, 77, "saved-password");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let old = fixture.install(&session, 1);
    configure(&fixture, 28081, 29092, 88, "current-source-password");
    let next = fixture.install(&session, 2);
    let mut active = old.clone();
    let promoted = switch_services_with_rollback(
        &mut active,
        |_| Ok(()),
        || {
            assert_eq!(app(&next.runtime_dir.join("api")).app.port, 28081);
            Err("candidate readiness failed".into())
        },
        || {
            let api = app(&old.runtime_dir.join("api"));
            let worker = app(&old.runtime_dir.join("worker"));
            assert_eq!((api.app.port, worker.jobs.health_port), (18081, 19092));
            assert_eq!(
                (api.snowflake_worker_id, worker.snowflake_worker_id),
                (77, 78)
            );
            assert_eq!(api.database.primary.password, "saved-password");
            Ok(old.clone())
        },
        || panic!("候选失败不得清理旧 LKG"),
    )
    .unwrap();
    assert!(!promoted);
    assert_eq!(active.generation_dir, old.generation_dir);
    assert!(old.api.is_file());
    assert!(old.worker.is_file());
}

#[test]
fn obsolete_or_unknown_manifest_fields_are_rejected_without_conversion_or_deletion() {
    for unknown in [false, true] {
        let fixture = SnapshotFixture::new("strict-manifest");
        let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
        let saved = fixture.install(&session, 3);
        let path = saved.generation_dir.join("manifest.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if unknown {
            document["unexpected"] = "must-not-echo-manifest-value".into();
        } else {
            document["format_version"] = 2.into();
        }
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let error = DevSession::prepare(&fixture.root, &DevOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("manifest"));
        assert!(!error.contains("must-not-echo-manifest-value"));
        assert!(saved.api.is_file());
        assert!(saved.runtime_dir.is_dir());
    }
}

#[test]
fn missing_or_tampered_private_input_is_not_recovered_or_deleted() {
    for missing in [false, true] {
        let fixture = SnapshotFixture::new("private-integrity");
        let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
        let older = fixture.install(&session, 1);
        let newest = fixture.install(&session, 2);
        let path = newest.runtime_dir.join("api/app.dev.toml");
        if missing {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, "[app]\nport=17070\n").unwrap();
        }
        let (_, recovered) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
        assert_eq!(recovered.unwrap().generation_dir, older.generation_dir);
        assert!(newest.api.is_file());
        assert!(newest.runtime_dir.is_dir());
    }
}

#[test]
fn recovery_seed_preserves_provenance_and_new_watcher_events_are_newer() {
    let fixture = SnapshotFixture::new("historical-revision");
    let (session, _) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    fixture.install(&session, 319);
    let (_, recovered) = DevSession::prepare(&fixture.root, &DevOptions::default()).unwrap();
    let mut saved = recovered.unwrap();
    assert_eq!(saved.source_revision.value(), 319);
    let seed = recovery_seed(Some(&saved)).unwrap();
    assert_eq!(seed.value(), 320);
    let watcher = SourceWatcher::new_with_inputs(
        &fixture.root,
        WatchInputRoots {
            config_dir: fixture.root.join("config"),
            locales_dir: fixture.root.join("locales"),
        },
        seed,
    )
    .unwrap();
    assert_eq!(watcher.current_revision(), seed);
    fs::write(
        fixture.root.join("config/app.dev.toml"),
        "[app]\nport=18082\n",
    )
    .unwrap();
    let event = watcher
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(matches!(event,crate::watch::WatchEvent::BackendChanged{revision,..} if revision>seed));
    assert_eq!(saved.source_revision.value(), 319);
    saved.source_revision = SourceRevision::from_value(u64::MAX);
    assert!(recovery_seed(Some(&saved)).is_err());
}

#[test]
fn dev_options_are_explicit_and_plan_does_not_resolve_nonexistent_inputs() {
    let parse = |args: &[&str]| cli::parse(args.iter().map(|arg| (*arg).to_owned()).collect());
    let cli = parse(&[
        "dev",
        "--plan",
        "--workspace-root",
        "missing-workspace",
        "--config-dir",
        "missing-config",
    ])
    .unwrap();
    assert!(
        matches!(cli.command,Command::Dev(options) if options.plan && options.workspace_root==Some(PathBuf::from("missing-workspace")))
    );
    let cli = parse(&[
        "dev",
        "--measure-once",
        "--case",
        "config-only",
        "--output",
        "result.json",
    ])
    .unwrap();
    assert!(
        matches!(cli.command,Command::Dev(options) if options.case==Some(SaveCase::ConfigOnly) && options.output==Some(PathBuf::from("result.json")))
    );
    for args in [
        vec!["dev", "--case", "api-only"],
        vec!["dev", "--output", "result.json"],
        vec!["dev", "--measure-once", "--case", "unknown"],
        vec!["dev", "--plan", "--plan"],
        vec!["dev", "--config-dir", "a", "--config-dir", "b"],
    ] {
        assert!(parse(&args).is_err());
    }
}
