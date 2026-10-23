//! 原生续作协议在无服务、无有效确认时失败关闭。
use super::{evidence_protocol, resume_model, resume_observations};
use serde_json::{Value, json};
use std::fs;

#[test]
fn confirmed_protocol_rejects_missing_duplicate_and_malformed_proof_readonly() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests/target-resume-evidence");
    fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let request = json!({"format_version":1,"operation":"migration-resume-confirmed",
        "backend":backend,"target":directory.path(),"initialized":{}});
    let invoke = || evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(invoke(), json!({"confirmed":false}));
    for attempt in ["a", "b"] {
        let path = directory.path().join(format!(
            "resume-initialize-{}.confirmed.json",
            attempt.repeat(32)
        ));
        fs::write(&path, b"{\"private\":\"diagnostic\"}").unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(invoke(), json!({"confirmed":false}));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn resume_protocol_requires_exact_fields_and_boolean_owned_lock() {
    let backend = crate::workspace::root_dir();
    let parent = backend.join(".local-tests/target-resume-protocol");
    fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let request = json!({"format_version":1,"operation":"migration-resume-state",
        "backend":backend,"target":directory.path(),"request_descriptor":{},
        "prepared_files":{},"owned_lock":false});
    for changed in [json!(0), json!(null), json!("false")] {
        let mut value = request.clone();
        value["owned_lock"] = changed;
        assert!(evidence_protocol::execute(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = request.clone();
    value["unregistered"] = json!(true);
    assert!(evidence_protocol::execute(&serde_json::to_vec(&value).unwrap()).is_err());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn original_failure_requires_exact_schema_and_literal_flags() {
    let value = json!({"status":"needs_reconciliation","stage":"create_and_reset",
        "at":"2026-10-23T00:00:00Z","error_type":"FileNotFoundError","reason":"private",
        "automatic_retry":false,"automatic_resource_cleanup":false,"fresh_target_initialized":false,
        "clone_verified":false,"restore_qualified":false});
    assert!(resume_model::failure(&value).unwrap());
    for key in [
        "automatic_retry",
        "automatic_resource_cleanup",
        "fresh_target_initialized",
        "clone_verified",
        "restore_qualified",
    ] {
        for changed in [json!(true), json!(0), Value::Null] {
            let mut changed_value = value.clone();
            changed_value[key] = changed;
            assert!(!resume_model::failure(&changed_value).unwrap());
        }
    }
    let mut changed = value;
    changed["extra"] = json!(false);
    assert!(resume_model::failure(&changed).is_err());
}

#[test]
fn proc_identity_ignores_volatile_counters_but_preserves_pid_comm_and_start() {
    let stable = format!("200 (redis-server) S {}67890\n", "0 ".repeat(18));
    let volatile = stable.replacen(" S 0 0 ", " R 41 73 ", 1);
    assert_eq!(
        resume_observations::process_identity(&stable).unwrap(),
        resume_observations::process_identity(&volatile).unwrap()
    );
    for changed in [
        stable.replacen("200", "201", 1),
        stable.replace("redis-server", "other"),
        stable.replace("67890", "67891"),
    ] {
        assert_ne!(
            resume_observations::process_identity(&stable).unwrap(),
            resume_observations::process_identity(&changed).unwrap()
        );
    }
    assert!(resume_observations::process_identity("200 (redis-server) S").is_err());
}

#[test]
fn initialization_tree_accepts_only_canonical_registered_directory_shapes() {
    let files = json!([
        {"path":"reset-state","type":"directory"},
        {"path":"reset-state/ledger.json","type":"file","bytes":0,"sha256":"a".repeat(64)}
    ]);
    assert_eq!(resume_observations::file_map(&files).unwrap().len(), 2);
    for name in [
        "../outside",
        "reset-state/../outside",
        "/outside",
        "reset-state//file",
        "D:/outside",
    ] {
        let mut changed = files.clone();
        changed[0]["path"] = json!(name);
        assert!(resume_observations::file_map(&changed).is_err());
    }
    let mut changed = files;
    changed[0]["bytes"] = json!(0);
    assert!(resume_observations::file_map(&changed).is_err());
}

#[test]
fn protected_binaries_preserve_each_frozen_artifact_key_order() {
    for roles in [
        ["reset", "migrate", "tenant-data"],
        ["migrate", "reset", "tenant-data"],
        ["tenant-data", "migrate", "reset"],
    ] {
        let entries = roles.map(|role| format!("\"{role}\":{{\"executable\":\"{role}\"}}"));
        let bytes = format!(
            "{{\"kind\":\"devex-clone-tool-build\",\"artifacts\":{{{}}}}}",
            entries.join(",")
        );
        let values = resume_model::artifact_bindings(bytes.as_bytes()).unwrap();
        assert_eq!(
            values
                .iter()
                .map(|value| value["executable"].as_str().unwrap())
                .collect::<Vec<_>>(),
            roles
        );
    }
    assert!(
        resume_model::artifact_bindings(
            br#"{"artifacts":{"reset":{},"reset":{},"migrate":{},"tenant-data":{}}}"#
        )
        .is_err()
    );
}
