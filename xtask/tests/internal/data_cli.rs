use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use super::cli::{
    BackupCommand, BackupInventoryOptions, BackupObservation, BackupRegisterOptions, Command,
    DataCommand, FileMaintenanceMode, FileMaintenanceOperation, FileMaintenanceOptions,
    ResetCommand, ResetExecuteOptions, RestoreBeginOptions, RestoreCommand, RestoreDataOptions,
    RestoreVerifyOptions, TargetInventoryOptions, parse,
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
        for directory in ["backup", "config", "runner", "output"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in [
            "manifest.json",
            "plan.json",
            "proof.json",
            "tests.json",
            "runtime.json",
            "target.json",
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

#[test]
fn parses_every_restore_operation_into_typed_requests() {
    let fixture = DataFixture::new();
    let config = fixture.path("config");
    let plan = fixture.path("plan.json");
    let running = fixture.path("output/running.json");
    let backup = fixture.path("backup");
    let data_output = fixture.path("output/data.json");
    let proof = fixture.path("proof.json");
    let tests = fixture.path("tests.json");
    let runtime = fixture.path("runtime.json");
    let target = fixture.path("target.json");
    let runner = fixture.path("runner");
    assert_eq!(
        command(&[
            "data",
            "restore",
            "begin",
            "--restore-config-dir",
            &config,
            "--output",
            &running,
            "--plan",
            &plan,
            "--write",
        ]),
        Command::Data(DataCommand::Restore(RestoreCommand::Begin(
            RestoreBeginOptions {
                plan: plan.into(),
                output: running.into(),
                restore_config_dir: config.clone().into(),
            }
        )))
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
            "--restore-config-dir",
            &config,
            "--write",
        ]),
        Command::Data(DataCommand::Restore(RestoreCommand::VerifyData(
            RestoreDataOptions {
                id: "drill-1".into(),
                backup_root: backup.into(),
                output: data_output.into(),
                restore_config_dir: config.clone().into(),
            }
        )))
    );
    assert_eq!(
        command(&[
            "data",
            "restore",
            "verify",
            "--id",
            "drill-1",
            "--proof",
            &proof,
            "--tests-receipt",
            &tests,
            "--runtime-receipt",
            &runtime,
            "--target-plan",
            &target,
            "--runner-root",
            &runner,
            "--restore-config-dir",
            &config,
            "--write",
        ]),
        Command::Data(DataCommand::Restore(RestoreCommand::Verify(
            RestoreVerifyOptions {
                id: "drill-1".into(),
                proof: proof.into(),
                tests_receipt: tests.into(),
                runtime_receipt: runtime.into(),
                target_plan: target.into(),
                runner_root: runner.into(),
                restore_config_dir: config.into(),
            }
        )))
    );
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
    #[cfg(unix)]
    fs::remove_file(&linked).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&linked).unwrap();
}

#[test]
fn parses_file_and_reset_operations_with_canonical_defaults() {
    assert_eq!(
        command(&[
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "ryframe_test",
        ]),
        Command::Data(DataCommand::File(FileMaintenanceOptions {
            operation: FileMaintenanceOperation::BackfillSha256,
            mode: FileMaintenanceMode::DryRun,
            database: "ryframe_test".into(),
            batch_size: 100,
            start_after: i64::MIN,
        }))
    );
    assert_eq!(
        command(&[
            "data",
            "file",
            "drain-legacy-reservations",
            "apply",
            "--database",
            "ryframe_test",
            "--batch-size",
            "25",
            "--start-after",
            "-7",
            "--confirm-apply",
            "APPLY-FILE-A-MAINTENANCE",
            "--write",
        ]),
        Command::Data(DataCommand::File(FileMaintenanceOptions {
            operation: FileMaintenanceOperation::DrainLegacyReservations,
            mode: FileMaintenanceMode::Apply,
            database: "ryframe_test".into(),
            batch_size: 25,
            start_after: -7,
        }))
    );
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
fn rejects_file_writes_before_starting_the_private_binary() {
    for values in [
        vec!["data", "file"],
        vec!["data", "file", "unknown", "dry-run"],
        vec!["data", "file", "backfill-sha256", "unknown"],
        vec!["data", "file", "backfill-sha256", "dry-run"],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "-h",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "db",
            "--batch-size",
            "0",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "db",
            "--batch-size",
            "1001",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "db",
            "--start-after",
            "invalid",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "dry-run",
            "--database",
            "db",
            "--confirm-apply",
            "APPLY-FILE-A-MAINTENANCE",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "apply",
            "--database",
            "db",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "apply",
            "--database",
            "db",
            "--confirm-apply",
            "wrong",
        ],
        vec![
            "data",
            "file",
            "backfill-sha256",
            "apply",
            "--database",
            "db",
            "--database",
            "other",
            "--confirm-apply",
            "APPLY-FILE-A-MAINTENANCE",
        ],
    ] {
        rejects(&values);
    }
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
