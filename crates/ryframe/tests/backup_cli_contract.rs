#[path = "../src/bin/ryframe_tenant_data/args.rs"]
mod args;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;
#[path = "../src/bin/ryframe_tenant_data/output.rs"]
mod output;
#[path = "../src/bin/ryframe_tenant_data/preimage.rs"]
mod preimage;
#[path = "../src/bin/ryframe_tenant_data/proof_binding.rs"]
mod proof_binding;
#[path = "backup_cli_contract/proof_binding.rs"]
mod proof_binding_contract;
#[path = "../src/bin/ryframe_tenant_data/proof_file.rs"]
mod proof_file;

use args::{Command, InventoryTime, parse};
use ryframe_kernel::AppError;
use serde_json::json;
use std::fs;

fn arguments(value: &str) -> Result<Command, String> {
    let arguments = ryframe_config::ConfigurationArgs::parse(
        value.split_whitespace().map(String::from).collect(),
    )
    .map_err(|error| error.to_string())?;
    parse(arguments.arguments)
}

fn configuration_directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("file-tenant-data-")
        .tempdir_in(root)
        .unwrap()
}

#[test]
fn registry_and_restore_configuration_directories_are_independent() {
    use ryframe_config::{AppConfig, ConfigurationArgs, Environment};

    let parsed = ConfigurationArgs::parse(
        [
            "--config-dir",
            "registry",
            "restore-begin",
            "--plan",
            "plan.json",
            "--output",
            "running.json",
            "--restore-config-dir",
            "restored",
        ]
        .map(String::from)
        .into(),
    )
    .unwrap();
    assert_eq!(parsed.directory, std::path::PathBuf::from("registry"));
    let command = parse(parsed.arguments).unwrap();
    assert_eq!(
        command.restore_config().unwrap(),
        &std::path::PathBuf::from("restored")
    );
    let temporary = configuration_directory();
    let registry = temporary.path().join("registry");
    let restored = temporary.path().join("restored");
    configuration_fixture::write_configuration(&registry, 8080, 9091, None);
    configuration_fixture::write_configuration(&restored, 8081, 9092, None);
    let source = AppConfig::load(&registry, Environment::Test).unwrap();
    let target = AppConfig::load(&restored, source.environment).unwrap();
    assert_eq!(source.app.port, 8080);
    assert_eq!(target.app.port, 8081);
}

#[test]
fn duplicate_registry_directory_does_not_consume_restore_directory_or_publish_output() {
    for command in [
        "--config-dir one --config-dir two backup-status",
        "backup-status --config-dir",
        "restore-begin --plan plan.json --output out.json --restore-config-dir target --config-dir one --config-dir two",
    ] {
        assert!(arguments(command).is_err(), "{command}");
    }
}

#[test]
fn offline_help_and_business_catalog_do_not_read_environment_or_configuration() {
    let temporary = configuration_directory();
    let missing = temporary.path().join("absent");
    for args in [
        vec!["--help"],
        vec!["business-catalog", "--output", "catalog.json"],
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_ryframe-tenant-data"))
            .current_dir(temporary.path())
            .arg("--config-dir")
            .arg(&missing)
            .args(args)
            .env("APP_ENV", "must-not-read-for-offline")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!missing.exists());
    }
    assert!(temporary.path().join("catalog.json").is_file());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
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
            "restore-verify --id drill --proof proof.json --tests-receipt tests.json --runtime-receipt runtime.json --target-plan target.json --coordinator-root coordinator --coordinator-sha aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --runner-root runner --restore-config-dir isolated"
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
fn restore_begin_requires_explicit_preimage_stdin_before_connecting() {
    let args = [
        "restore-begin",
        "--plan",
        "absent-plan.json",
        "--output",
        "running.json",
        "--restore-config-dir",
        "absent-target",
    ];
    let base = args.join(" ");
    assert!(matches!(
        arguments(&format!("{base} --target-preimage-stdin")).unwrap(),
        Command::Begin {
            target_preimage_stdin: true,
            ..
        }
    ));
    for command in [
        format!("{base} --target-preimage-stdin --target-preimage-stdin"),
        format!("{base} --target-preimage-stdin true"),
        "backup-status --target-preimage-stdin".into(),
    ] {
        assert!(arguments(&command).is_err(), "{command}");
    }
    let directory = configuration_directory();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ryframe-tenant-data"))
        .current_dir(directory.path())
        .args(["--config-dir", "absent-registry"])
        .args(args)
        .env("APP_ENV", "must-not-read-for-offline")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains(preimage::required().message())
    );
    assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
}

#[test]
fn restore_preimage_stdin_requires_one_bounded_utf8_object_with_exact_fields() {
    let json = serde_json::to_string(&json!({
        "format_version": 1,
        "plan_hash": "a".repeat(64),
        "target_plan_sha256": "b".repeat(64),
        "initialized_sha256": "c".repeat(64),
        "resource_image_sha256": "d".repeat(64),
        "databases": [{
            "target_key": "shared-control",
            "tables": [{"table": "biz_example", "rows": 0, "sha256": "e".repeat(64)}],
        }],
    }))
    .unwrap();
    let received = preimage::read(json.as_bytes()).unwrap();
    assert_eq!(received.databases[0].target_key, "shared-control");
    assert!(preimage::read([0xff].as_slice()).is_err());
    for malformed in [
        format!("{json} {{}}"),
        json.replacen('{', "{\"format_version\":1,", 1),
        json.replacen('{', "{\"unknown\":1,", 1),
    ] {
        assert!(preimage::read(malformed.as_bytes()).is_err());
    }
    let oversized = std::io::Read::take(std::io::repeat(b' '), 16 * 1024 * 1024 + 1);
    assert!(
        preimage::read(oversized)
            .unwrap_err()
            .message()
            .contains("16 MiB")
    );
}

#[test]
fn restore_begin_registry_must_be_disjoint_from_every_planned_physical_database() {
    let plan = serde_json::from_value(json!({
        "id": "drill",
        "backup_id": "backup",
        "scope_id": "scope",
        "fault_at": "2026-01-01T00:00:00Z",
        "databases": [
            {"source_key": "base", "target_key": "base", "server_uuid": "one", "database": "db1"},
            {"source_key": "shared", "target_key": "shared", "server_uuid": "two", "database": "db2"},
        ],
        "object_endpoint": "127.0.0.1:9000",
        "object_prefix": "scope/",
        "api_ready_url": "http://127.0.0.1:8080/readyz",
        "worker_ready_url": "http://127.0.0.1:9091/readyz",
        "frontend_sha": "a".repeat(40),
    }))
    .unwrap();
    assert!(preimage::require_registry_disjoint(&plan, "one", "DB1").is_err());
    assert!(preimage::require_registry_disjoint(&plan, "two", "DB2").is_err());
    assert!(preimage::require_registry_disjoint(&plan, "two", "DB1").is_ok());
    assert!(preimage::require_registry_disjoint(&plan, "two", "registry").is_ok());
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
    let original = bytes;
    assert!(output::validate_new_output(&output).is_err());
    assert!(output::publish_json(&output, &json!({"changed": true})).is_err());
    assert_eq!(fs::read(&output).unwrap(), original);

    fs::write(&output, b"{}\n").unwrap();
    assert!(output::verify_published(&output, &value, &original).is_err());
}

#[cfg(windows)]
mod windows_output {
    use super::output::{publish_json, validate_new_output};
    use std::{fs, path::Path};

    #[test]
    fn publishes_a_long_windows_filename_without_leaving_staging() {
        let local = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let parent = directory.path().canonicalize().unwrap();
        let name = format!("{}.json", "x".repeat(245));
        let output = parent.join(&name);
        let value = serde_json::json!({"status": "running"});

        assert_eq!(validate_new_output(&output).unwrap(), output);
        assert_eq!(publish_json(&output, &value).unwrap(), output);
        assert_eq!(
            fs::read(&output).unwrap(),
            b"{\n  \"status\": \"running\"\n}\n"
        );
        let files = fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(files, [std::ffi::OsString::from(name)]);
    }
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

#[test]
fn backup_inventory_preserves_legacy_capture_bytes() {
    use chrono::{DateTime, Utc};
    use ryframe_application::ports::utc_timestamp;

    let temporary = configuration_directory();
    let started_at = DateTime::parse_from_rfc3339("2026-11-25T18:05:00.400000+08:00")
        .unwrap()
        .with_timezone(&Utc);
    for (index, (input, legacy_capture)) in [
        ("2026-11-25T10:06:00.000000Z", "2026-11-25T10:06:00Z"),
        ("2026-11-25T10:06:00.120000Z", "2026-11-25T10:06:00.120Z"),
        ("2026-11-25T10:06:00.123400Z", "2026-11-25T10:06:00.123400Z"),
    ]
    .into_iter()
    .enumerate()
    {
        let captured_at = DateTime::parse_from_rfc3339(input)
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            serde_json::to_string(&captured_at).unwrap(),
            format!("\"{legacy_capture}\"")
        );
        for (key, observation) in [
            ("quiesced_at", InventoryTime::QuiescedAt(started_at)),
            ("observed_at", InventoryTime::ObservedAt(started_at)),
        ] {
            let inventory = output::Inventory {
                scope_id: "fixture".into(),
                source_sha: "0123456789abcdef0123456789abcdef01234567".into(),
                observation,
                captured_at: utc_timestamp::from_chrono(captured_at).unwrap(),
                control_schema_fingerprint: "control".into(),
                tenant_schema_fingerprint: "tenant".into(),
                databases: vec![],
                objects: vec![],
            };
            let path = temporary.path().join(format!("{key}-{index}.json"));
            output::write_inventory(&path, &inventory).unwrap();
            let expected = format!(
                r#"{{
  "scope_id": "fixture",
  "source_sha": "0123456789abcdef0123456789abcdef01234567",
  "{key}": "2026-11-25T10:05:00.400Z",
  "captured_at": "{legacy_capture}",
  "control_schema_fingerprint": "control",
  "tenant_schema_fingerprint": "tenant",
  "databases": [],
  "objects": []
}}"#
            );
            let actual = fs::read(path).unwrap();
            assert_eq!(actual.last(), Some(&b'}'));
            assert_eq!(actual.as_slice(), expected.as_bytes());
        }
    }
}
