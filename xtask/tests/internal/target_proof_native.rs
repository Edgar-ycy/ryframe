use std::{fs, path::PathBuf, process::Command};

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
fn initialized_history_matches_python_and_includes_resume_evidence() {
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
    let output = Command::new(crate::check::consumer_context::python::selected_program(&workspace::root_dir()).unwrap())
        .args(["-X", "utf8", "-B", "-c", "import json,sys; from pathlib import Path; from devex_clone_target import history; print(json.dumps(history(Path(sys.argv[1]))))"])
        .arg(&fixture.target)
        .env("PYTHONPATH", fixture.backend.join("tools/python"))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        before,
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    );
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
