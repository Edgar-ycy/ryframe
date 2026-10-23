use super::*;

fn fixture() -> tempfile::TempDir {
    let base = crate::workspace::root_dir().join(".local-tests/source-runtime-inventory");
    fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn write(root: &Path, name: &str, value: &Value) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    documents::write_new(&path, value).unwrap();
}

fn inputs(root: &Path) -> Value {
    Value::Array(evidence_files(root).unwrap())
}

#[test]
fn manifest_is_sorted_preserves_empty_files_and_rejects_empty_nested_directories() {
    let temporary = fixture();
    let root = temporary.path();
    write(root, "z.json", &json!({"z":true}));
    write(root, "a/b.json", &json!({"a":true}));
    fs::write(root.join("a/empty"), []).unwrap();
    let rows = manifest(root).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a/b.json", "a/empty", "z.json"]
    );
    assert_eq!(rows[1]["bytes"], 0);
    fs::create_dir(root.join("a/unknown")).unwrap();
    assert!(manifest(root).is_err());
}

#[test]
fn registered_inputs_remain_immutable_and_unknown_files_fail_closed() {
    let temporary = fixture();
    let root = temporary.path();
    write(root, "before/image.json", &json!({"image":true}));
    write(root, "audit/login-before.tsv", &json!({"rows":true}));
    let frozen = inputs(root);
    assert!(verify_inputs(root, &frozen).is_ok());
    write(root, "after/image.json", &json!({"image":true}));
    write(root, "failed.json", &json!({"status":"failed"}));
    assert!(verify_inputs(root, &frozen).is_ok());
    write(root, "after/unknown.json", &json!({"unknown":true}));
    assert!(verify_inputs(root, &frozen).is_err());
    fs::remove_file(root.join("after/unknown.json")).unwrap();
    fs::write(root.join("before/image.json"), b"changed").unwrap();
    assert!(verify_inputs(root, &frozen).is_err());
}

#[test]
fn stage_roots_can_be_empty_but_unregistered_directories_cannot() {
    let temporary = fixture();
    let root = temporary.path();
    fs::create_dir(root.join("before")).unwrap();
    assert!(evidence_files(root).unwrap().is_empty());
    assert!(manifest(root).is_err());
    fs::create_dir(root.join("before/nested")).unwrap();
    assert!(evidence_files(root).is_err());
    fs::remove_dir(root.join("before/nested")).unwrap();
    fs::create_dir(root.join("unknown")).unwrap();
    assert!(evidence_files(root).is_err());
}

#[test]
fn recovery_queries_require_intent_and_same_control_database_command() {
    let temporary = fixture();
    let root = temporary.path();
    let original = format!("audit/mysql-shared-control-{}.command.json", "a".repeat(32));
    let successor = format!(
        "recovery-audit/mysql-shared-control-{}.command.json",
        "b".repeat(32)
    );
    let query = json!({"command":["mysql","--defaults-file=bound.cnf"],"returncode":0,"error_type":null,"stdout":"rows","stderr":""});
    write(root, &original, &query);
    let frozen = inputs(root);
    write(root, &successor, &query);
    assert!(verify_inputs(root, &frozen).is_err());
    write(
        root,
        "source-verification-recovery-intent.json",
        &json!({"intent":true}),
    );
    assert!(verify_inputs(root, &frozen).is_ok());
    let mut changed = query;
    changed["command"][1] = json!("--defaults-file=another.cnf");
    fs::write(root.join(&successor), serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(verify_inputs(root, &frozen).is_err());
}

fn kernel(started: &str, cpu: &str) -> Value {
    let mut fields = vec!["0"; 20];
    fields[0] = "R";
    fields[10] = cpu;
    fields[19] = started;
    json!({"command":["docker","exec","redis","/usr/bin/cat","/proc/77/stat"],
        "returncode":0,"error_type":null,"stdout":format!("77 (redis server) {}",fields.join(" ")),"stderr":""})
}

#[test]
fn kernel_counters_may_change_but_creation_identity_and_complete_counts_must_match() {
    let temporary = fixture();
    let root = temporary.path();
    write(root, "before/image.json", &json!({"complete":true}));
    for index in 0..10 {
        write(
            root,
            &format!("before/redis-kernel-{index:032x}.command.json"),
            &kernel("900", "12"),
        );
    }
    let frozen = inputs(root);
    for index in 10..19 {
        write(
            root,
            &format!("after/redis-kernel-{index:032x}.command.json"),
            &kernel("900", "99"),
        );
    }
    assert!(verify_inputs(root, &frozen).is_ok());
    write(root, "after/image.json", &json!({"complete":true}));
    assert!(verify_inputs(root, &frozen).is_err());
    write(
        root,
        &format!("after/redis-kernel-{:032x}.command.json", 19),
        &kernel("900", "99"),
    );
    assert!(verify_inputs(root, &frozen).is_ok());
    fs::write(
        root.join(format!("after/redis-kernel-{:032x}.command.json", 19)),
        serde_json::to_vec(&kernel("901", "99")).unwrap(),
    )
    .unwrap();
    assert!(verify_inputs(root, &frozen).is_err());
}

#[test]
fn database_stdout_requires_both_diagnostic_and_command_binding_at_completion() {
    let temporary = fixture();
    let root = temporary.path();
    write(root, "before/image.json", &json!({"complete":true}));
    for (stage, offset) in [("before", 0), ("after", 100)] {
        for (category, count) in [("identity", 16), ("ownership", 24)] {
            for index in 0..count {
                let name = format!("mysql-{category}-{:032x}", index + offset);
                let stdout = root.join(format!("{stage}/databases/{name}.stdout"));
                fs::create_dir_all(stdout.parent().unwrap()).unwrap();
                fs::write(&stdout, format!("{category}-{index}")).unwrap();
                let descriptor = common::binding(&stdout).unwrap();
                write(
                    root,
                    &format!("{stage}/databases/{name}.json"),
                    &json!({
                    "format_version":1,"kind":"mysql-verification-output","check":category,
                    "target_key":"shared-control","returncode":0,"error_type":null,
                    "stderr_bytes":0,"stderr_sha256":hex::encode(sha2::Sha256::digest([])),"stdout_file":descriptor}),
                );
                let ordinal = if category == "ownership" {
                    16 + index
                } else {
                    index
                };
                write(
                    root,
                    &format!("{stage}/databases/command-{:032x}.json", ordinal + offset),
                    &json!({
                    "command":["mysql","--read-only"],"returncode":0,"error_type":null,"stdin":null,
                    "stdout":"","stderr":"","stdout_file":descriptor}),
                );
            }
        }
        for index in 40..48 {
            let phase = if index < 44 { "before" } else { "after" };
            let key = ["shared-control", "shared", "dedicated-a", "dedicated-b"][(index - 40) % 4];
            write(
                root,
                &format!("{stage}/databases/command-{:032x}.json", index + offset),
                &json!({
                    "command":["migrate","--output",root.join(format!("{stage}/databases/{phase}-target-{key}.json"))],
            "returncode":0,"error_type":null,"stdin":null,"stdout":"","stderr":""}),
            );
        }
    }
    let frozen = Value::Array(
        evidence_files(root)
            .unwrap()
            .into_iter()
            .filter(|row| row["path"].as_str().unwrap().starts_with("before/"))
            .collect(),
    );
    write(root, "after/image.json", &json!({"complete":true}));
    verify_inputs(root, &frozen).unwrap();
    fs::write(
        root.join(format!(
            "after/databases/mysql-identity-{:032x}.stdout",
            100
        )),
        b"different",
    )
    .unwrap();
    assert!(verify_inputs(root, &frozen).is_err());
}

use sha2::Digest;
