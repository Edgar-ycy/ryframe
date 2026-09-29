use std::path::PathBuf;

use super::{
    cli::{
        BackupCommand, BackupInventoryOptions, BackupObservation, BackupRegisterOptions,
        FileMaintenanceMode, FileMaintenanceOperation, FileMaintenanceOptions, ResetCommand,
        ResetExecuteOptions, RestoreBeginOptions, RestoreCommand, RestoreDataOptions,
        RestoreVerifyOptions, TargetInventoryOptions,
    },
    data::arguments::{
        backup_arguments, file_arguments, reset_arguments, restore_arguments,
        target_inventory_arguments,
    },
};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn backup_and_target_commands_rebuild_private_arguments_in_stable_order() {
    assert_eq!(
        backup_arguments(&BackupCommand::Inventory(BackupInventoryOptions {
            output: PathBuf::from("D:/备份 空格/inventory.json"),
            source_sha: "a".repeat(40),
            observation: BackupObservation::QuiescedAt("2026-09-12T08:00:00+08:00".into()),
        })),
        strings(&[
            "backup-inventory",
            "--output",
            "D:/备份 空格/inventory.json",
            "--source-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--quiesced-at",
            "2026-09-12T08:00:00+08:00",
        ])
    );
    assert_eq!(
        backup_arguments(&BackupCommand::Register(BackupRegisterOptions {
            manifest: "manifest.json".into(),
            backup_root: "D:/backup root".into(),
        })),
        strings(&[
            "backup-register",
            "--manifest",
            "manifest.json",
            "--backup-root",
            "D:/backup root",
        ])
    );
    assert_eq!(backup_arguments(&BackupCommand::Status), ["backup-status"]);
    assert_eq!(
        target_inventory_arguments(&TargetInventoryOptions {
            target: "tenant-a".into(),
            output: "target.json".into(),
        }),
        strings(&[
            "target-inventory",
            "--target",
            "tenant-a",
            "--output",
            "target.json",
        ])
    );
}

#[test]
fn restore_commands_rebuild_every_required_path_without_forwarding_raw_argv() {
    assert_eq!(
        restore_arguments(&RestoreCommand::Begin(RestoreBeginOptions {
            plan: "plan.json".into(),
            output: "running.json".into(),
            restore_config_dir: "D:/隔离 config".into(),
        })),
        strings(&[
            "restore-begin",
            "--plan",
            "plan.json",
            "--output",
            "running.json",
            "--restore-config-dir",
            "D:/隔离 config",
        ])
    );
    assert_eq!(
        restore_arguments(&RestoreCommand::VerifyData(RestoreDataOptions {
            id: "drill-1".into(),
            backup_root: "backup".into(),
            output: "verified.json".into(),
            restore_config_dir: "config".into(),
        })),
        strings(&[
            "restore-verify-data",
            "--id",
            "drill-1",
            "--backup-root",
            "backup",
            "--output",
            "verified.json",
            "--restore-config-dir",
            "config",
        ])
    );
    assert_eq!(
        restore_arguments(&RestoreCommand::Verify(RestoreVerifyOptions {
            id: "drill-1".into(),
            proof: "proof.json".into(),
            tests_receipt: "tests.json".into(),
            runtime_receipt: "runtime.json".into(),
            target_plan: "target.json".into(),
            runner_root: "runner".into(),
            restore_config_dir: "config".into(),
        })),
        strings(&[
            "restore-verify",
            "--id",
            "drill-1",
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
        ])
    );
}

#[test]
fn file_and_reset_commands_rebuild_only_validated_write_arguments() {
    assert_eq!(
        file_arguments(&FileMaintenanceOptions {
            operation: FileMaintenanceOperation::BackfillSha256,
            mode: FileMaintenanceMode::DryRun,
            database: "ryframe_test".into(),
            batch_size: 100,
            start_after: i64::MIN,
        }),
        strings(&[
            "backfill-sha256",
            "dry-run",
            "--database",
            "ryframe_test",
            "--batch-size",
            "100",
            "--start-after",
            "-9223372036854775808",
        ])
    );
    assert_eq!(
        file_arguments(&FileMaintenanceOptions {
            operation: FileMaintenanceOperation::DrainLegacyReservations,
            mode: FileMaintenanceMode::Apply,
            database: "ryframe_test".into(),
            batch_size: 2,
            start_after: 9,
        }),
        strings(&[
            "drain-legacy-reservations",
            "apply",
            "--database",
            "ryframe_test",
            "--batch-size",
            "2",
            "--start-after",
            "9",
            "--confirm-apply",
            "APPLY-FILE-A-MAINTENANCE",
        ])
    );
    assert_eq!(reset_arguments(&ResetCommand::Plan), ["plan"]);
    assert_eq!(
        reset_arguments(&ResetCommand::Execute(ResetExecuteOptions {
            plan_hash: "b".repeat(64),
            confirmation: "RESET scope database".into(),
        })),
        strings(&[
            "execute",
            "--plan-hash",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--confirm-reset",
            "RESET scope database",
        ])
    );
}
