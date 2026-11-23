use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::cli::{
    BackupCommand, BackupInventoryOptions, BackupObservation, BackupRegisterOptions, Command,
    DataCommand, ResetCommand, ResetExecuteOptions, RestoreBeginOptions, RestoreCommand,
    RestoreDataOptions, RestoreVerifyOptions, TargetInventoryOptions, parse,
};

fn command(values: &[&str]) -> Command {
    parse(values.iter().map(|value| (*value).to_owned()).collect())
        .unwrap()
        .command
}

fn rejects(values: &[&str]) {
    assert!(
        parse(values.iter().map(|value| (*value).to_owned()).collect()).is_err(),
        "参数本应在 xtask 首层拒绝：{values:?}"
    );
}

fn rejects_owned(values: Vec<String>) {
    assert!(
        parse(values.clone()).is_err(),
        "参数本应在 xtask 首层拒绝：{values:?}"
    );
}

struct DataFixture {
    root: PathBuf,
}

impl DataFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = super::workspace::root_dir()
            .join(".local-tests")
            .join(format!(
                "xtask-data-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        for directory in ["backup", "config", "output"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "manifest.json",
            "plan.json",
            "proof.json",
            "target.json",
            "runtime.json",
            "build.json",
        ] {
            fs::write(root.join(file), b"{}\n").unwrap();
        }
        Self { root }
    }

    fn path(&self, value: &str) -> String {
        self.root.join(value).to_string_lossy().into_owned()
    }
}

impl Drop for DataFixture {
    fn drop(&mut self) {
        let boundary = super::workspace::root_dir().join(".local-tests");
        assert!(self.root.starts_with(&boundary) && self.root != boundary);
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn parses_every_backup_and_target_operation_into_typed_requests() {
    let fixture = DataFixture::new();
    let inventory = fixture.path("output/inventory.json");
    let observed = fixture.path("output/observed.json");
    let manifest = fixture.path("manifest.json");
    let backup = fixture.path("backup");
    let target_output = fixture.path("output/target.json");
    assert_eq!(
        command(&[
            "data",
            "backup",
            "inventory",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--quiesced-at",
            "2026-09-12T08:00:00+08:00",
            "--output",
            &inventory,
        ]),
        Command::Data(DataCommand::Backup(BackupCommand::Inventory(
            BackupInventoryOptions {
                output: PathBuf::from(&inventory),
                source_sha: "a".repeat(40),
                observation: BackupObservation::QuiescedAt("2026-09-12T08:00:00+08:00".into()),
            }
        )))
    );
    assert!(matches!(
        command(&[
            "data",
            "backup",
            "inventory",
            "--output",
            &observed,
            "--source-sha",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--observed-at",
            "2026-09-12T00:00:00Z",
        ]),
        Command::Data(DataCommand::Backup(BackupCommand::Inventory(
            BackupInventoryOptions {
                observation: BackupObservation::ObservedAt(_),
                ..
            }
        )))
    ));
    assert_eq!(
        command(&[
            "data",
            "backup",
            "register",
            "--backup-root",
            &backup,
            "--manifest",
            &manifest,
            "--write",
        ]),
        Command::Data(DataCommand::Backup(BackupCommand::Register(
            BackupRegisterOptions {
                manifest: manifest.into(),
                backup_root: backup.into(),
            }
        )))
    );
    assert_eq!(
        command(&["data", "backup", "status"]),
        Command::Data(DataCommand::Backup(BackupCommand::Status))
    );
    assert_eq!(
        command(&[
            "data",
            "target",
            "inventory",
            "--output",
            &target_output,
            "--target",
            "tenant-a",
        ]),
        Command::Data(DataCommand::TargetInventory(TargetInventoryOptions {
            target: "tenant-a".into(),
            output: target_output.into(),
        }))
    );
}

#[test]
fn rejects_incomplete_or_ambiguous_backup_requests() {
    for values in [
        vec!["data", "backup"],
        vec!["data", "backup", "unknown"],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "inventory.json",
            "--source-sha",
            "short",
            "--quiesced-at",
            "2026-09-12T00:00:00Z",
        ],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "inventory.json",
            "--source-sha",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--quiesced-at",
            "2026-09-12T00:00:00Z",
        ],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "inventory.json",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "inventory.json",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--quiesced-at",
            "bad-date",
        ],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "inventory.json",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--quiesced-at",
            "2026-09-12T00:00:00Z",
            "--observed-at",
            "2026-09-12T00:00:00Z",
        ],
        vec![
            "data",
            "backup",
            "inventory",
            "--output",
            "one.json",
            "--output",
            "two.json",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--observed-at",
            "2026-09-12T00:00:00Z",
        ],
        vec!["data", "backup", "register", "--manifest", "manifest.json"],
        vec![
            "data",
            "backup",
            "register",
            "--manifest",
            "manifest.json",
            "--backup-root",
            "backup",
            "--unknown",
            "value",
        ],
        vec!["data", "backup", "status", "--id", "unexpected"],
    ] {
        rejects(&values);
    }
}

#[test]
fn rejects_placeholder_or_malformed_source_and_plan_hashes() {
    let fixture = DataFixture::new();
    for source_sha in [
        "short",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "0000000000000000000000000000000000000000",
    ] {
        rejects_owned(vec![
            "data".into(),
            "backup".into(),
            "inventory".into(),
            "--output".into(),
            fixture.path("output/invalid-source.json"),
            "--source-sha".into(),
            source_sha.into(),
            "--observed-at".into(),
            "2026-09-12T00:00:00Z".into(),
        ]);
    }
    for plan_hash in [
        "short",
        "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        "0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        rejects_owned(vec![
            "data".into(),
            "reset".into(),
            "execute".into(),
            "--plan-hash".into(),
            plan_hash.into(),
            "--confirm-reset".into(),
            "RESET scope database".into(),
            "--write".into(),
        ]);
    }
}

#[test]
fn rejects_incomplete_or_ambiguous_target_requests() {
    for values in [
        vec!["data", "target"],
        vec!["data", "target", "unknown"],
        vec!["data", "target", "inventory", "--target", "tenant-a"],
        vec![
            "data",
            "target",
            "inventory",
            "--target",
            "tenant-a",
            "--output",
            "one.json",
            "--output",
            "two.json",
        ],
    ] {
        rejects(&values);
    }
}

#[test]
fn rejects_target_keys_outside_the_configured_identity_syntax() {
    let fixture = DataFixture::new();
    for target in [
        "a",
        "-tenant",
        "tenant-",
        "tenant key",
        "租户-a",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        rejects_owned(vec![
            "data".into(),
            "target".into(),
            "inventory".into(),
            "--target".into(),
            target.into(),
            "--output".into(),
            fixture.path("output/invalid-target.json"),
        ]);
    }
}

const RESTORE_ID: &str = "drill-1";
const RESTORE_PROOF: &str = "restore-drill-1.json";
const RESTORE_TESTS: &str = "restore-drill-1-tests.json";
const RESTORE_RUNTIME: &str = "restore-drill-1-runtime.json";

struct RestoreRunner(tempfile::TempDir);

impl RestoreRunner {
    fn new() -> Self {
        let backend = super::workspace::root_dir();
        let directory = tempfile::Builder::new()
            .prefix("xtask-restore-runner-")
            .tempdir_in(backend.parent().expect("后端工作树缺少父目录"))
            .unwrap();
        let fixture = Self(directory);
        fs::create_dir_all(fixture.receipt(RESTORE_PROOF).parent().unwrap()).unwrap();
        for file in [RESTORE_PROOF, RESTORE_TESTS, RESTORE_RUNTIME] {
            fs::write(fixture.receipt(file), b"{}\n").unwrap();
        }
        fixture
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn receipt(&self, filename: &str) -> PathBuf {
        self.root()
            .join(".local-tests/playwright-real")
            .join(filename)
    }

    fn verify_args(&self, backend: &DataFixture) -> Vec<String> {
        vec![
            "data".into(),
            "restore".into(),
            "verify".into(),
            "--id".into(),
            RESTORE_ID.into(),
            "--proof".into(),
            self.receipt(RESTORE_PROOF).to_string_lossy().into_owned(),
            "--tests-receipt".into(),
            self.receipt(RESTORE_TESTS).to_string_lossy().into_owned(),
            "--runtime-receipt".into(),
            self.receipt(RESTORE_RUNTIME).to_string_lossy().into_owned(),
            "--target-plan".into(),
            backend.path("target.json"),
            "--coordinator-root".into(),
            super::workspace::root_dir().to_string_lossy().into_owned(),
            "--coordinator-sha".into(),
            "a".repeat(40),
            "--runner-root".into(),
            self.root().to_string_lossy().into_owned(),
            "--restore-config-dir".into(),
            backend.path("config"),
            "--write".into(),
        ]
    }
}

#[test]
fn parses_every_restore_operation_into_typed_requests() {
    let fixture = DataFixture::new();
    let config = fixture.path("config");
    let plan = fixture.path("plan.json");
    let running = fixture.path("uncreated-reference-work/running.json");
    let backup = fixture.path("backup");
    let target = fixture.path("target.json");
    let runtime = fixture.path("runtime.json");
    let maintenance_root = fixture.path("config");
    let maintenance_build = fixture.path("build.json");
    let data_output = fixture.path("output/data.json");
    let runner = RestoreRunner::new();
    assert_eq!(
        command(&[
            "data",
            "restore",
            "begin",
            "--output",
            &running,
            "--plan",
            &plan,
            "--backup-root",
            &backup,
            "--target-plan",
            &target,
            "--runtime-registration",
            &runtime,
            "--maintenance-root",
            &maintenance_root,
            "--maintenance-build",
            &maintenance_build,
            "--write",
        ]),
        Command::Data(DataCommand::Restore(RestoreCommand::Begin(Box::new(
            RestoreBeginOptions {
                plan: plan.into(),
                backup_root: backup.clone().into(),
                target_plan: target.into(),
                runtime_registration: runtime.into(),
                maintenance_root: maintenance_root.clone().into(),
                maintenance_build: maintenance_build.into(),
                output: running.into(),
                copy: None,
            }
        ))))
    );
    assert_eq!(
        command(&[
            "data",
            "restore",
            "verify-data",
            "--id",
            "drill-1",
            "--backup-root",
            &backup,
            "--output",
            &data_output,
            "--config-dir",
            &maintenance_root,
            "--restore-config-dir",
            &config,
            "--write",
        ]),
        Command::Data(DataCommand::Restore(RestoreCommand::VerifyData(
            RestoreDataOptions {
                id: "drill-1".into(),
                backup_root: backup.into(),
                output: data_output.into(),
                config_dir: Some(maintenance_root.into()),
                restore_config_dir: config.clone().into(),
            }
        )))
    );
    let mut expected = RestoreVerifyOptions {
        id: RESTORE_ID.into(),
        proof: runner.receipt(RESTORE_PROOF),
        tests_receipt: runner.receipt(RESTORE_TESTS),
        runtime_receipt: runner.receipt(RESTORE_RUNTIME),
        target_plan: fixture.path("target.json").into(),
        coordinator_root: super::workspace::root_dir(),
        coordinator_sha: "a".repeat(40),
        runner_root: runner.root().to_path_buf(),
        config_dir: None,
        restore_config_dir: config.into(),
    };
    let mut verify = runner.verify_args(&fixture);
    assert_eq!(
        parse(verify.clone()).unwrap().command,
        Command::Data(DataCommand::Restore(RestoreCommand::Verify(Box::new(
            expected.clone()
        ))))
    );
    let source_config = runner.root().join("source registry");
    fs::create_dir(&source_config).unwrap();
    verify.extend([
        "--config-dir".into(),
        source_config.to_string_lossy().into_owned(),
    ]);
    expected.config_dir = Some(source_config);
    assert_eq!(
        parse(verify).unwrap().command,
        Command::Data(DataCommand::Restore(RestoreCommand::Verify(Box::new(
            expected
        ))))
    );
}

#[test]
fn restore_verify_requires_exact_runner_receipts_and_coordinator_plan() {
    let backend = DataFixture::new();
    let runner = RestoreRunner::new();
    let other = RestoreRunner::new();
    let valid = runner.verify_args(&backend);
    assert!(parse(valid.clone()).is_ok());
    let reject = |flag: &str, path: &Path| {
        let mut args = valid.clone();
        let index = args.iter().position(|value| value == flag).unwrap();
        args[index + 1] = path.to_string_lossy().into_owned();
        rejects_owned(args);
    };

    reject("--runner-root", other.root());
    reject("--proof", &other.receipt(RESTORE_PROOF));
    for (flag, wrong_name) in [
        ("--proof", "restore-drill-2.json"),
        ("--tests-receipt", "restore-drill-2-tests.json"),
        ("--runtime-receipt", "restore-drill-2-runtime.json"),
    ] {
        let wrong_id = runner.receipt(wrong_name);
        fs::write(&wrong_id, b"{}\n").unwrap();
        reject(flag, &wrong_id);
    }
    let wrong_directory = runner.root().join(".local-tests").join(RESTORE_PROOF);
    fs::write(&wrong_directory, b"{}\n").unwrap();
    reject("--proof", &wrong_directory);

    reject("--runner-root", Path::new("relative-runner"));
    reject("--coordinator-root", Path::new("relative-coordinator"));
    reject(
        "--coordinator-root",
        &runner.root().join("missing-coordinator"),
    );
    reject("--coordinator-root", other.root());
    reject("--runner-root", &runner.root().join("missing-runner"));
    reject("--proof", Path::new(RESTORE_PROOF));
    let runner_target = runner.root().join(".local-tests/target.json");
    fs::write(&runner_target, b"{}\n").unwrap();
    reject("--target-plan", &runner_target);

    for source_config in [
        PathBuf::from("relative-config"),
        runner.root().join("missing-config"),
        runner.receipt(RESTORE_PROOF),
        runner.root().join(".."),
    ] {
        let mut args = valid.clone();
        args.extend([
            "--config-dir".into(),
            source_config.to_string_lossy().into_owned(),
        ]);
        rejects_owned(args);
    }
    let mut duplicate = valid.clone();
    duplicate.extend([
        "--config-dir".into(),
        backend.path("config"),
        "--config-dir".into(),
        backend.path("backup"),
    ]);
    rejects_owned(duplicate);

    fs::remove_file(runner.receipt(RESTORE_RUNTIME)).unwrap();
    rejects_owned(valid);
}

#[test]
fn restore_verify_binds_external_coordinator_plan_without_changing_invocation_root() {
    let backend = DataFixture::new();
    let runner = RestoreRunner::new();
    let coordinator = RestoreRunner::new();
    let plan = coordinator.root().join(".local-tests/target.json");
    fs::write(&plan, b"{}\n").unwrap();
    let mut arguments = runner.verify_args(&backend);
    for (flag, value) in [
        ("--coordinator-root", coordinator.root()),
        ("--target-plan", plan.as_path()),
    ] {
        let index = arguments.iter().position(|value| value == flag).unwrap();
        arguments[index + 1] = value.to_string_lossy().into_owned();
    }
    let parsed = parse(arguments.clone()).unwrap();
    let Command::Data(DataCommand::Restore(RestoreCommand::Verify(options))) = parsed.command
    else {
        panic!("应解析为恢复核验命令");
    };
    assert_eq!(options.coordinator_root, coordinator.root());
    assert_eq!(options.target_plan, plan);
    assert!(options.config_dir.is_none());
    assert!(
        !options
            .target_plan
            .starts_with(super::workspace::root_dir())
    );
    for flag in ["--coordinator-root", "--coordinator-sha"] {
        let mut missing = arguments.clone();
        let index = missing.iter().position(|value| value == flag).unwrap();
        missing.drain(index..index + 2);
        assert!(parse(missing).unwrap_err().to_string().contains(flag));
    }
    for invalid in ["HEAD".into(), "A".repeat(40), "0".repeat(40)] {
        let mut changed = arguments.clone();
        let index = changed
            .iter()
            .position(|value| value == "--coordinator-sha")
            .unwrap();
        changed[index + 1] = invalid;
        assert!(
            parse(changed)
                .unwrap_err()
                .to_string()
                .contains("--coordinator-sha")
        );
    }
}

#[test]
fn restore_begin_accepts_a_paired_negative_copy_source_and_id() {
    let fixture = DataFixture::new();
    let options = command(&[
        "data",
        "restore",
        "begin",
        "--plan",
        &fixture.path("plan.json"),
        "--backup-root",
        &fixture.path("backup"),
        "--target-plan",
        &fixture.path("target.json"),
        "--runtime-registration",
        &fixture.path("runtime.json"),
        "--maintenance-root",
        &fixture.path("config"),
        "--maintenance-build",
        &fixture.path("build.json"),
        "--output",
        &fixture.path("uncreated-work/negative.json"),
        "--copy-source-plan",
        &fixture.path("proof.json"),
        "--copy-id",
        "negative-missing",
        "--write",
    ]);
    let Command::Data(DataCommand::Restore(RestoreCommand::Begin(begin))) = options else {
        panic!("应解析为开始登记");
    };
    let copy = begin.copy.expect("应包含副本绑定");
    assert_eq!(copy.id, "negative-missing");
    assert_eq!(copy.publisher_plan, fixture.root.join("proof.json"));
}

#[test]
fn rejects_incomplete_duplicate_or_unknown_restore_arguments() {
    for values in [
        vec!["data", "restore"],
        vec!["data", "restore", "unknown"],
        vec![
            "data",
            "restore",
            "begin",
            "--plan",
            "plan.json",
            "--output",
            "running.json",
        ],
        vec![
            "data",
            "restore",
            "begin",
            "--plan",
            "one.json",
            "--plan",
            "two.json",
            "--output",
            "running.json",
            "--restore-config-dir",
            "config",
        ],
        vec![
            "data",
            "restore",
            "verify-data",
            "--id",
            "drill",
            "--backup-root",
            "backup",
            "--output",
            "data.json",
        ],
        vec![
            "data",
            "restore",
            "verify",
            "--id",
            "drill",
            "--proof",
            "proof.json",
            "--tests-receipt",
            "tests.json",
            "--runtime-receipt",
            "runtime.json",
            "--target-plan",
            "target.json",
            "--runner-root",
            "runner",
        ],
        vec![
            "data",
            "restore",
            "verify",
            "--id",
            "drill",
            "--proof",
            "proof.json",
            "--tests-receipt",
            "tests.json",
            "--runtime-receipt",
            "runtime.json",
            "--target-plan",
            "target.json",
            "--runner-root",
            "runner",
            "--restore-config-dir",
            "config",
            "--unknown",
            "value",
        ],
    ] {
        rejects(&values);
    }
}

#[test]
fn write_markers_and_real_path_roles_are_checked_before_dispatch() {
    let fixture = DataFixture::new();
    let manifest = fixture.path("manifest.json");
    let backup = fixture.path("backup");
    rejects_owned(vec![
        "data".into(),
        "backup".into(),
        "register".into(),
        "--manifest".into(),
        manifest.clone(),
        "--backup-root".into(),
        backup.clone(),
    ]);
    rejects_owned(vec![
        "data".into(),
        "backup".into(),
        "register".into(),
        "--manifest".into(),
        manifest,
        "--backup-root".into(),
        backup,
        "--write".into(),
        "--write".into(),
    ]);
    rejects_owned(vec![
        "data".into(),
        "backup".into(),
        "inventory".into(),
        "--output".into(),
        fixture.path("output/inventory-with-write.json"),
        "--source-sha".into(),
        "a".repeat(40),
        "--observed-at".into(),
        "2026-09-12T00:00:00Z".into(),
        "--write".into(),
    ]);

    let outside = std::env::temp_dir().join(format!("outside-data-{}.json", std::process::id()));
    rejects_owned(vec![
        "data".into(),
        "target".into(),
        "inventory".into(),
        "--target".into(),
        "tenant-a".into(),
        "--output".into(),
        outside.to_string_lossy().into_owned(),
    ]);
    rejects(&[
        "data",
        "target",
        "inventory",
        "--target",
        "tenant-a",
        "--output",
        ".local-tests/relative.json",
    ]);

    let existing_output = fixture.path("proof.json");
    rejects_owned(vec![
        "data".into(),
        "target".into(),
        "inventory".into(),
        "--target".into(),
        "tenant-a".into(),
        "--output".into(),
        existing_output,
    ]);
    let missing_input = fixture.path("missing.json");
    rejects_owned(vec![
        "data".into(),
        "backup".into(),
        "register".into(),
        "--manifest".into(),
        missing_input,
        "--backup-root".into(),
        fixture.path("backup"),
        "--write".into(),
    ]);
}

#[test]
fn registered_external_directories_are_not_forced_under_local_tests() {
    let fixture = DataFixture::new();
    let external = super::workspace::root_dir()
        .join("target")
        .join(format!("xtask-data-external-{}", std::process::id()));
    fs::create_dir_all(&external).unwrap();
    let parsed = command(&[
        "data",
        "backup",
        "register",
        "--manifest",
        &fixture.path("manifest.json"),
        "--backup-root",
        &external.to_string_lossy(),
        "--write",
    ]);
    assert!(matches!(
        parsed,
        Command::Data(DataCommand::Backup(BackupCommand::Register(_)))
    ));
    fs::remove_dir(&external).unwrap();
}

#[test]
fn linked_data_paths_are_rejected_when_the_platform_can_create_a_link() {
    let fixture = DataFixture::new();
    let linked = fixture.root.join("linked-output");
    #[cfg(windows)]
    let linked_result = std::os::windows::fs::symlink_dir(fixture.root.join("output"), &linked);
    #[cfg(unix)]
    let linked_result = std::os::unix::fs::symlink(fixture.root.join("output"), &linked);
    if linked_result.is_err() {
        return;
    }
    rejects_owned(vec![
        "data".into(),
        "target".into(),
        "inventory".into(),
        "--target".into(),
        "tenant-a".into(),
        "--output".into(),
        linked.join("target.json").to_string_lossy().into_owned(),
    ]);
    let runner = RestoreRunner::new();
    let mut verify = runner.verify_args(&fixture);
    verify.extend(["--config-dir".into(), linked.to_string_lossy().into_owned()]);
    rejects_owned(verify);
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
}

#[test]
fn parses_reset_operations_with_canonical_defaults() {
    assert_eq!(
        command(&["data", "reset", "plan"]),
        Command::Data(DataCommand::Reset(ResetCommand::Plan))
    );
    assert_eq!(
        command(&[
            "data",
            "reset",
            "execute",
            "--confirm-reset",
            "RESET scope database",
            "--plan-hash",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--write",
        ]),
        Command::Data(DataCommand::Reset(ResetCommand::Execute(
            ResetExecuteOptions {
                plan_hash: "b".repeat(64),
                confirmation: "RESET scope database".into(),
            }
        )))
    );
}

#[test]
fn rejects_reset_writes_before_starting_the_private_binary() {
    for values in [
        vec!["data", "reset"],
        vec!["data", "reset", "plan", "extra"],
        vec!["data", "reset", "unknown"],
        vec![
            "data",
            "reset",
            "execute",
            "--plan-hash",
            "short",
            "--confirm-reset",
            "RESET",
        ],
        vec![
            "data",
            "reset",
            "execute",
            "--plan-hash",
            "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            "--confirm-reset",
            "RESET",
        ],
        vec![
            "data",
            "reset",
            "execute",
            "--plan-hash",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ],
        vec![
            "data",
            "reset",
            "execute",
            "--plan-hash",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--plan-hash",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "--confirm-reset",
            "RESET",
        ],
    ] {
        rejects(&values);
    }
}

#[test]
fn unknown_data_operation_does_not_dispatch_a_binary() {
    rejects(&["data", "file"]);
}
