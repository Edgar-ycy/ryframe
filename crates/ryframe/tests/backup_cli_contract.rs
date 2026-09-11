#[path = "../src/bin/ryframe_tenant_data/args.rs"]
mod args;
#[path = "../src/bin/ryframe_tenant_data/output.rs"]
mod output;
#[path = "../src/bin/ryframe_tenant_data/proof_file.rs"]
mod proof_file;

use args::{Command, InventoryTime, parse};
use ryframe_kernel::AppError;
use serde_json::json;
use std::{fs, thread, time::Duration};

fn arguments(value: &str) -> Result<Command, String> {
    parse(value.split_whitespace().map(String::from))
}

#[test]
fn artifact_source_detects_same_file_same_length_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("python.exe");
    std::fs::write(&path, b"before").unwrap();
    let original = proof_file::stable_artifact(&path, "测试解释器").unwrap();
    assert_eq!(original.bytes, 6);
    let result = proof_file::stable_artifact_with_hook(&path, "测试解释器", || {
        thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, b"after!").unwrap();
    });
    assert!(result.is_err());
}

#[test]
fn explicit_commands_require_scoped_inputs_and_reject_duplicate_flags() {
    assert_eq!(arguments("--help").unwrap(), Command::Help);
    assert_eq!(arguments("backup-status").unwrap(), Command::Status);
    assert!(arguments("backup-register --target old --provider-ref old").is_err());
    assert!(arguments("backup-register --manifest m.json").is_err());
    assert!(
        arguments("backup-register --manifest m.json --manifest n.json --backup-root data")
            .is_err()
    );
    assert!(arguments("backup-status --backup-root data").is_err());
    let command = arguments(
        "restore-verify-data --id drill --backup-root data --output verified.json --restore-config-dir isolated",
    )
    .unwrap();
    assert_eq!(command.restore_config().unwrap().to_str(), Some("isolated"));
    assert_eq!(command.backup_root().to_str(), Some("data"));
    assert!(
        arguments("restore-verify --id drill --proof proof.json --restore-config-dir isolated")
            .is_err()
    );
    assert!(
        arguments(
            "restore-verify --id drill --proof proof.json --tests-receipt tests.json --runtime-receipt runtime.json --target-plan target.json --runner-root runner --restore-config-dir isolated"
        )
        .is_ok()
    );
    assert!(args::USAGE.contains("外部工具"));
}

#[test]
fn restore_records_require_outputs_without_touching_existing_files_on_argument_failure() {
    assert!(arguments("restore-begin --plan plan.json --restore-config-dir isolated").is_err());
    assert!(
        arguments(
            "restore-verify-data --id drill --backup-root data --restore-config-dir isolated"
        )
        .is_err()
    );
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("running.json");
    let absent = directory.path().join("absent.json");
    let invalid_absent = format!(
        "restore-begin --plan plan.json --output {} --restore-config-dir isolated --unknown value",
        absent.display()
    );
    assert!(arguments(&invalid_absent).is_err());
    assert!(!absent.exists());
    fs::write(&output, b"keep").unwrap();
    let invalid = format!(
        "restore-begin --plan plan.json --output {} --restore-config-dir isolated --unknown value",
        output.display()
    );
    assert!(arguments(&invalid).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"keep");
}

#[test]
fn restore_record_publication_is_create_new_and_canonical() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("恢复 record.json");
    let value = json!({"z": 1, "nested": {"b": false, "a": "值"}});
    let published = output::publish_json(&output, &value).unwrap();
    assert_eq!(published, output.canonicalize().unwrap());
    let bytes = fs::read(&output).unwrap();
    assert!(bytes.ends_with(b"\n"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        value
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let original = bytes.clone();
    assert!(output::validate_new_output(&output).is_err());
    assert!(output::publish_json(&output, &json!({"changed": true})).is_err());
    assert_eq!(fs::read(&output).unwrap(), original);

    fs::write(&output, b"{}\n").unwrap();
    assert!(output::verify_published(&output, &value, &original).is_err());
}

#[test]
fn business_failure_keeps_original_error_and_does_not_publish_a_record() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("failed.json");
    let error = output::publish_result::<serde_json::Value>(
        &output,
        Err(AppError::Conflict("原始业务失败".into())),
        "恢复开始记录",
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "数据冲突: 原始业务失败");
    assert!(!output.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn target_inventory_requires_one_explicit_target_and_output() {
    assert_eq!(
        arguments("target-inventory --target unused --output target.json").unwrap(),
        Command::TargetInventory {
            target: "unused".into(),
            output: "target.json".into()
        }
    );
    for command in [
        "target-inventory",
        "target-inventory --output target.json",
        "target-inventory --target unused",
        "target-inventory --target unused --target another --output target.json",
        "target-inventory --target unused --output target.json --all true",
        "target-inventory --target unused --output target.json --restore-config-dir other",
    ] {
        assert!(arguments(command).is_err(), "{command}");
    }
}

#[test]
fn inventory_requires_one_observation_kind_and_serializes_only_that_fact() {
    #[derive(serde::Serialize)]
    struct Receipt {
        #[serde(flatten)]
        observation: InventoryTime<String>,
    }
    let base = "backup-inventory --output inventory.json --source-sha head";
    for (flag, observation, field) in [
        (
            "--quiesced-at",
            InventoryTime::QuiescedAt("time".into()),
            "quiesced_at",
        ),
        (
            "--observed-at",
            InventoryTime::ObservedAt("time".into()),
            "observed_at",
        ),
    ] {
        assert_eq!(
            arguments(&format!("{base} {flag} time")).unwrap(),
            Command::Inventory {
                output: "inventory.json".into(),
                source_sha: "head".into(),
                observation: observation.clone(),
            }
        );
        assert_eq!(
            serde_json::to_value(Receipt { observation }).unwrap(),
            serde_json::json!({field: "time"})
        );
        assert!(arguments(&format!("{base} {flag} time {flag} other")).is_err());
    }
    assert!(arguments(base).is_err());
    assert!(arguments(&format!("{base} --quiesced-at time --observed-at time")).is_err());
}
