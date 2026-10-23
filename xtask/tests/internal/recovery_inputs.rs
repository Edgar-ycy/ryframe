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
    let serialized = serde_json::to_string(&reference).unwrap();
    for secret in [
        "access_key",
        "secret_key",
        "source-access",
        "source-secret",
        "target-access",
        "target-secret",
    ] {
        assert!(!serialized.contains(secret));
    }
    for (side, directory) in [
        ("source", fixture.root.join("source-runtime").join("config")),
        ("target", fixture.root.join("config")),
    ] {
        let binding = &reference[side]["s3"]["configuration"];
        assert_eq!(binding["app_env"], "test");
        assert_eq!(
            fs::canonicalize(binding["directory"].as_str().unwrap()).unwrap(),
            fs::canonicalize(directory).unwrap()
        );
    }
    let source_request = read_json(&fixture.root.join("source-request.json"));
    let path = fixture.root.join("source-request.json");
    let before = fs::read(&path).unwrap();
    let descriptor =
        document::binding(&document::read(&path, &fixture.backend, "私有源请求").unwrap());
    let projected = reference::source_projection(&fixture.backend, &descriptor).unwrap();
    assert_eq!(projected.as_object().unwrap().len(), 2);
    assert_eq!(projected["source"], reference["source"]);
    assert_eq!(projected["tools"], reference["tools"]);
    assert_eq!(source_request["source"]["s3"].as_object().unwrap().len(), 4);
    assert_eq!(projected["source"]["s3"].as_object().unwrap().len(), 3);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(source_request["tools"].as_object().unwrap().len(), 3);
    assert!(source_request["tools"].get("node").is_some());
    assert_eq!(reference["tools"].as_object().unwrap().len(), 2);
    assert!(reference["tools"].get("node").is_none());
    for name in ["mysql", "mysqldump"] {
        assert_eq!(reference["tools"][name], source_request["tools"][name]);
    }
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

#[test]
fn reference_configuration_drift_rejects_before_publication() {
    let fixture = Fixture::new();
    let mut options = fixture.reference_options(None);
    options.work_dir = fixture.root.join("work").join("configuration-drift");
    for directory in [
        fixture.root.join("source-runtime").join("config"),
        fixture.root.join("config"),
    ] {
        let path = directory.join("app.test.toml");
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"[object_storage]\nsecret_key='changed'\n").unwrap();
        assert!(reference::build(&fixture.backend, &options).is_err());
        assert!(!fixture.output.exists());
        assert!(!options.work_dir.exists());
        fs::write(path, original).unwrap();
    }
    reference::build(&fixture.backend, &options).unwrap();
}

#[test]
fn source_reference_projection_rejects_binding_runtime_and_configuration_drift() {
    let fixture = Fixture::new();
    let path = fixture.root.join("source-request.json");
    let request = document::read(&path, &fixture.backend, "私有源请求").unwrap();
    let descriptor = document::binding(&request);
    let mut bad = descriptor.clone();
    bad["sha256"] = Value::String("0".repeat(64));
    assert!(reference::source_projection(&fixture.backend, &bad).is_err());
    bad = descriptor.clone();
    bad["unexpected"] = Value::Bool(true);
    assert!(reference::source_projection(&fixture.backend, &bad).is_err());

    let runtime = std::path::Path::new(request.value["runtime"]["path"].as_str().unwrap());
    let runtime_before = fs::read(runtime).unwrap();
    fs::write(runtime, b"{}").unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    fs::write(runtime, &runtime_before).unwrap();

    let config = fixture.root.join("source-runtime/config/app.test.toml");
    let config_before = fs::read(&config).unwrap();
    let mut changed = config_before.clone();
    changed.push(b'\n');
    fs::write(&config, changed).unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    fs::write(&config, &config_before).unwrap();

    let mut wrong = request.value.clone();
    wrong["source"]["s3"]["secret_key"] = Value::String("changed-private-value".into());
    fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(reference::source_projection(&fixture.backend, &descriptor).is_err());
    let rebound =
        document::binding(&document::read(&path, &fixture.backend, "私有源请求").unwrap());
    let error = reference::source_projection(&fixture.backend, &rebound)
        .unwrap_err()
        .to_string();
    assert!(error.contains("与绑定私有请求不一致"));
    assert!(!error.contains("changed-private-value"));
    fs::write(&path, &request.raw).unwrap();
    reference::source_projection(&fixture.backend, &descriptor).unwrap();
    assert_eq!(fs::read(runtime).unwrap(), runtime_before);
    assert_eq!(fs::read(config).unwrap(), config_before);
    assert_eq!(fs::read(path).unwrap(), request.raw);
}
