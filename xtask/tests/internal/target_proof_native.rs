use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use crate::{
    recovery::clone::native::{digest, json as json_io, target_verify::proof},
    workspace,
};

struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    target: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let parent = backend.join(".local-tests/target-proof");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        Self {
            backend,
            _directory: directory,
            target,
        }
    }

    fn history(&self) -> Vec<String> {
        let mut names = [
            "initialize.started.json",
            "prepare.json",
            "request.json",
            "reset-plan.json",
            "reset-plan-confirm.json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        for stage in [
            "sentinel",
            "reset",
            "create-shared-control",
            "create-shared",
            "create-dedicated-a",
            "create-dedicated-b",
        ] {
            for state in ["intent", "confirmed"] {
                names.push(format!("{stage}.{state}.json"));
            }
        }
        for name in &names {
            fs::write(
                self.target.join(name),
                format!("{{\"evidence\":\"{name}\"}}\n"),
            )
            .unwrap();
        }
        names
    }

    fn inventory(&self, directory: &str, name: &str, raw: &str) -> Value {
        let parent = self.target.join(directory);
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join(name);
        let raw_path = parent.join(raw);
        fs::create_dir_all(raw_path.parent().unwrap()).unwrap();
        fs::write(&raw_path, b"captured inventory\n").unwrap();
        let actual = digest::file(&raw_path).unwrap();
        json_io::write_new(
            &path,
            &json!({"inventories": {
                raw: {"bytes": actual.bytes, "sha256": actual.sha256},
            }}),
        )
        .unwrap();
        binding(&path)
    }
}

fn binding(path: &std::path::Path) -> Value {
    let actual = digest::file(path).unwrap();
    json!({"path": path, "bytes": actual.bytes, "sha256": actual.sha256})
}

#[test]
fn initialized_history_binds_every_file_and_includes_resume_evidence() {
    let fixture = Fixture::new();
    fixture.history();
    for name in [
        "failure.json",
        "failure-retry.json",
        "resume-prepare-id.confirmed.json",
        "resume-initialize-id.intent.json",
        "resume-initialize-id.started.json",
        "resume-initialize-id.failure.json",
        "resume-redis-markers-id.json",
        "resume-migrate-id.json",
        "resume-reset-plan.json",
        "resume-reset-plan-id.json",
    ] {
        fs::write(fixture.target.join(name), b"{}\n").unwrap();
    }
    let before = proof::history(&fixture.backend, &fixture.target).unwrap();
    for (name, binding) in before.as_object().unwrap() {
        let actual = digest::file(&fixture.target.join(name)).unwrap();
        assert_eq!(
            binding,
            &json!({"bytes": actual.bytes, "sha256": actual.sha256})
        );
    }
    assert_eq!(
        proof::history(&fixture.backend, &fixture.target).unwrap(),
        before
    );
    assert_eq!(before.as_object().unwrap().len(), 27);
}

#[test]
fn initialized_history_refuses_every_missing_mandatory_file() {
    let fixture = Fixture::new();
    for name in fixture.history() {
        let path = fixture.target.join(&name);
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(
            proof::history(&fixture.backend, &fixture.target).is_err(),
            "{name}"
        );
        fs::write(path, bytes).unwrap();
    }
}

#[test]
fn initial_inventory_requires_exact_filename_and_generation_directory() {
    let fixture = Fixture::new();
    let valid = fixture.inventory(
        "inventory-initial",
        "inventory.json",
        "before-target-shared.json",
    );
    let before = fs::read(valid["path"].as_str().unwrap()).unwrap();
    proof::initial_inventory(&fixture.backend, &fixture.target, &valid, false).unwrap();
    assert_eq!(fs::read(valid["path"].as_str().unwrap()).unwrap(), before);
    for (directory, name) in [
        ("other", "inventory.json"),
        ("nested/inventory-initial", "inventory.json"),
        ("inventory-initial", "other.json"),
    ] {
        let receipt = fixture.inventory(directory, name, "data.json");
        assert!(
            proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, true).is_err()
        );
    }
}

#[test]
fn initial_inventory_resume_requires_explicit_state_and_direct_target_parent() {
    let fixture = Fixture::new();
    let receipt = fixture.inventory("inventory-resume-id", "inventory.json", "data.json");
    assert!(proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).is_err());
    proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, true).unwrap();
    let receipt = fixture.inventory(
        "elsewhere/inventory-resume-id",
        "inventory.json",
        "data.json",
    );
    assert!(proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, true).is_err());
}

#[test]
fn initial_inventory_refuses_nested_raw_files_drift_and_unknown_binding_fields() {
    let fixture = Fixture::new();
    let mut receipt = fixture.inventory("inventory-initial", "inventory.json", "nested/data.json");
    assert!(proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).is_err());
    let path = PathBuf::from(receipt["path"].as_str().unwrap());
    let raw = fixture.target.join("inventory-initial/data.json");
    fs::write(&raw, b"original").unwrap();
    let actual = digest::file(&raw).unwrap();
    fs::write(
        &path,
        serde_json::to_vec(&json!({"inventories": {"data.json": {
            "bytes": actual.bytes, "sha256": actual.sha256,
        }}}))
        .unwrap(),
    )
    .unwrap();
    receipt = binding(&path);
    proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).unwrap();
    fs::write(&raw, b"changed!").unwrap();
    assert!(proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).is_err());
    receipt["unknown"] = json!(true);
    assert!(proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).is_err());
}

#[test]
fn empty_inventory_file_requires_numeric_byte_count() {
    let fixture = Fixture::new();
    let original = fixture.inventory("inventory-initial", "inventory.json", "empty.json");
    let receipt_path = PathBuf::from(original["path"].as_str().unwrap());
    let raw = fixture.target.join("inventory-initial/empty.json");
    fs::write(&raw, b"").unwrap();
    let hash = digest::file(&raw).unwrap().sha256;
    for bytes in [json!(0), Value::Null, json!(false), json!("0")] {
        fs::write(
            &receipt_path,
            serde_json::to_vec(&json!({"inventories":{
            "empty.json":{"bytes":bytes,"sha256":hash}}}))
            .unwrap(),
        )
        .unwrap();
        let receipt = binding(&receipt_path);
        assert_eq!(
            proof::initial_inventory(&fixture.backend, &fixture.target, &receipt, false).is_ok(),
            bytes == json!(0),
            "{bytes}"
        );
    }
}

#[test]
fn native_evidence_protocol_requires_exact_shape_and_bound_inventory() {
    use crate::recovery::clone::native::target_verify::evidence_protocol;
    let fixture = Fixture::new();
    let receipt = fixture.inventory("inventory-initial", "inventory.json", "data.json");
    let request = json!({"format_version":1,"operation":"initial-inventory",
        "backend":fixture.backend,"target":fixture.target,"receipt":receipt,"resumed":false});
    let observed = evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(
        observed,
        json!({"status":"initial_inventory_verified","remote_writes":0})
    );
    for case in 0..5 {
        let mut value = request.clone();
        match case {
            0 => value["format_version"] = json!(true),
            1 => value["resumed"] = json!(1),
            2 => value["operation"] = json!("unknown"),
            3 => value["credentials"] = json!({"password":"must-not-be-read"}),
            _ => value["receipt"]["sha256"] = json!("0".repeat(64)),
        }
        assert!(
            evidence_protocol::execute(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{case}"
        );
    }
}
