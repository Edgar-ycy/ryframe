use std::fs;

use serde_json::Value;

use super::cli::{RecoveryInputsCommand, RestoreInputPublication};
use super::recovery::inputs::{bindings, document, product, publish, reference, run};
use super::recovery_inputs_fixture::Fixture;

#[test]
fn reference_product_and_bindings_native_chains_succeed() {
    let fixture = Fixture::new();

    let reference = fixture.reference_value.clone();
    assert_eq!(reference, fixture.reference_value);
    assert_eq!(reference["target_side"], "base");
    assert_eq!(
        reference["target"]["worker_ready_url"],
        "http://127.0.0.1:18081/readyz"
    );

    let product = product::build(&fixture.backend, &fixture.product_options(None)).unwrap();
    assert_eq!(product["id"], "restore-case-a");
    assert_eq!(product["backup_id"], "backup-case-a");
    assert_eq!(product["frontend_sha"], "b".repeat(40));
    assert_eq!(product["object_prefix"], "base-a/");

    let bindings = bindings::build(&fixture.backend, &fixture.bindings_options(None)).unwrap();
    assert_eq!(bindings["record"], read_json(&fixture.record));
    assert_eq!(bindings["manifest"]["id"], reference["id"]);
    assert_eq!(bindings["manifest"]["source_sha"], "a".repeat(40));
}

#[test]
fn dry_run_writes_nothing_and_publication_is_create_only() {
    let fixture = Fixture::new();
    let dry_work = fixture.root.join("work").join("dry-reference");
    let mut options = fixture.reference_options(None);
    options.work_dir = dry_work.clone();
    let command = RecoveryInputsCommand::Reference(options.clone());
    run(&command, &fixture.backend).unwrap();
    assert!(!fixture.output.exists());
    assert!(!dry_work.exists());

    options.publication = Some(RestoreInputPublication {
        output: fixture.output.clone(),
    });
    let publish = RecoveryInputsCommand::Reference(options);
    run(&publish, &fixture.backend).unwrap();
    let original = fs::read(&fixture.output).unwrap();
    let result = run(&publish, &fixture.backend);
    assert!(result.is_err());
    assert_eq!(fs::read(&fixture.output).unwrap(), original);
    let pending = fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_string_lossy().contains(&format!(
                ".{}",
                fixture.output.file_name().unwrap().to_string_lossy()
            ))
        })
        .collect::<Vec<_>>();
    assert!(pending.is_empty(), "create-only 冲突后应清理临时文件");
}

#[test]
fn path_escape_missing_and_unknown_structures_are_rejected() {
    let fixture = Fixture::new();
    let mut escaped = fixture.reference_options(None);
    escaped.work_dir = fixture
        .backend
        .join(".local-tests")
        .join("..")
        .join("escaped");
    assert!(reference::build(&fixture.backend, &escaped).is_err());

    let mut missing = fixture.reference_options(None);
    missing.arm_input = fixture.root.join("missing.json");
    assert!(reference::build(&fixture.backend, &missing).is_err());

    let mut unknown = read_json(&fixture.arm_input);
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), Value::Bool(true));
    let unknown_path = fixture.root.join("arm-unknown.json");
    fs::write(&unknown_path, serde_json::to_vec_pretty(&unknown).unwrap()).unwrap();
    let mut invalid = fixture.reference_options(None);
    invalid.arm_input = unknown_path;
    assert!(reference::build(&fixture.backend, &invalid).is_err());
}

#[test]
fn invalid_identifier_and_utc_time_are_rejected() {
    let fixture = Fixture::new();
    let mut invalid_id = fixture.reference_options(None);
    invalid_id.id = "Upper-Case".into();
    assert!(reference::build(&fixture.backend, &invalid_id).is_err());

    let mut invalid_time = fixture.product_options(None);
    invalid_time.fault_at = "2026-10-18T00:00:00+08:00".into();
    assert!(product::build(&fixture.backend, &invalid_time).is_err());
}

#[test]
fn publication_requires_create_only_file_semantics() {
    let fixture = Fixture::new();
    let target = fixture.root.join("direct-output.json");
    let value = fixture.reference_value.clone();
    let mut command = fixture.reference_options(Some(target.clone()));
    command.work_dir = fixture.root.join("work").join("direct-work");
    command.publication = Some(RestoreInputPublication {
        output: target.clone(),
    });
    run(&RecoveryInputsCommand::Reference(command), &fixture.backend).unwrap();
    assert!(target.is_file());
    let racy = fixture.root.join("racy-output.json");
    fs::write(&racy, b"external writer").unwrap();
    let mut race = fixture.reference_options(Some(racy.clone()));
    race.work_dir = fixture.root.join("work").join("race-work");
    race.publication = Some(RestoreInputPublication {
        output: racy.clone(),
    });
    assert!(run(&RecoveryInputsCommand::Reference(race), &fixture.backend,).is_err());
    assert_eq!(fs::read(&racy).unwrap(), b"external writer");
    assert_eq!(value["format_version"], 1);
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn bindings_reject_a_different_explicit_backup_and_unverified_record() {
    let fixture = Fixture::new();
    let other = fixture.root.join("other-backup.json");
    fs::copy(&fixture.backup_receipt, &other).unwrap();
    let mut options = fixture.bindings_options(None);
    options.backup_receipt = other;
    assert!(bindings::build(&fixture.backend, &options).is_err());
    let mut record = read_json(&fixture.record);
    record["status"] = Value::String("started".into());
    fs::write(&fixture.record, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(bindings::build(&fixture.backend, &fixture.bindings_options(None)).is_err());
}

#[test]
fn reference_rejects_side_generation_and_runtime_overlap() {
    let fixture = Fixture::new();
    let mut options = fixture.reference_options(None);
    options.side = super::cli::RestoreInputSide::Candidate;
    assert!(reference::build(&fixture.backend, &options).is_err());
    options = fixture.reference_options(None);
    let runtime = std::path::PathBuf::from(
        fixture.reference_value["target"]["runtime_dir"]
            .as_str()
            .unwrap(),
    );
    fs::create_dir_all(&runtime).unwrap();
    options.work_dir = runtime.join("restore");
    assert!(reference::build(&fixture.backend, &options).is_err());
    let mut arm = read_json(&fixture.arm_input);
    arm["source_generation"] = serde_json::json!({"changed": true});
    fs::write(&fixture.arm_input, serde_json::to_vec(&arm).unwrap()).unwrap();
    assert!(reference::build(&fixture.backend, &fixture.reference_options(None)).is_err());
}

#[test]
fn publication_preserves_post_write_drift_and_cleans_pending_files() {
    let fixture = Fixture::new();
    let calls = std::cell::Cell::new(0);
    let value = serde_json::json!({"id": "original"});
    let result = publish::finish("reference", Some(&fixture.output), &value, || {
        calls.set(calls.get() + 1);
        Ok(if calls.get() == 1 {
            value.clone()
        } else {
            serde_json::json!({"id": "changed"})
        })
    });
    assert!(result.unwrap_err().to_string().contains("发布后"));
    assert_eq!(read_json(&fixture.output), value);
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
}

#[test]
fn publication_rejects_truncated_reread_and_preserves_concurrent_writer() {
    let fixture = Fixture::new();
    let value = serde_json::json!({"message": "中文", "id": 1});
    let content = document::canonical_bytes(&value).unwrap();
    fs::write(&fixture.output, &content[..content.len() - 1]).unwrap();
    assert!(document::verify_published(&fixture.output, &value, &content).is_err());
    fs::remove_file(&fixture.output).unwrap();
    let result = publish::finish("reference", Some(&fixture.output), &value, || {
        fs::write(&fixture.output, b"concurrent unrelated writer").unwrap();
        Ok(value.clone())
    });
    assert!(result.unwrap_err().to_string().contains("并发创建"));
    assert_eq!(
        fs::read(&fixture.output).unwrap(),
        b"concurrent unrelated writer"
    );
    assert!(!fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending")
    }));
}

#[test]
fn document_rejects_duplicate_keys_and_invalid_utf8() {
    let fixture = Fixture::new();
    let path = fixture.root.join("invalid.json");
    for bytes in [br#"{"side":"base","side":"candidate"}"#.as_slice(), &[0xff]] {
        fs::write(&path, bytes).unwrap();
        assert!(document::read(&path, &fixture.backend, "输入").is_err());
    }
}
