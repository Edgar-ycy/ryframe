#[path = "../src/bin/ryframe_tenant_data/args.rs"]
mod args;
#[path = "../src/bin/ryframe_tenant_data/proof_file.rs"]
mod proof_file;

use args::{Command, InventoryTime, parse};
use std::{thread, time::Duration};

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
        "restore-verify-data --id drill --backup-root data --restore-config-dir isolated",
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
