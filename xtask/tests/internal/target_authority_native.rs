use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::recovery::clone::native::{
    digest, json as json_io,
    target_verify::{authority, tree},
};

// 此夹具只验证私有协议与实际文件绑定；完整初始化历史另由 Python 对照测试覆盖。
struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    run: PathBuf,
    target: PathBuf,
    value: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests/target-authority-native");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(&parent).unwrap();
        let root = directory.path();
        let run = root.join("run");
        let target = root.join("target");
        fs::create_dir(&run).unwrap();
        fs::create_dir(&target).unwrap();
        fs::create_dir(target.join("empty-directory")).unwrap();
        fs::write(target.join("copy-target.guard"), [0]).unwrap();
        for name in [
            "source-request.json",
            "environment.json",
            "request.json",
            "initialized-files.json",
        ] {
            write(&root.join(name), &json!({"structural_fixture": true}));
        }
        write(
            &target.join("request.json"),
            &json!({"structural_fixture": true}),
        );
        write(
            &target.join("prepare.json"),
            &json!({"request": bound(&root.join("request.json"))}),
        );
        let generation = json!({"scope_id": "authority-fixture", "storage": {"fixture": true}});
        write(
            &target.join("initialized.json"),
            &json!({"generation": generation}),
        );
        write(
            &run.join("manifest.json"),
            &json!({
                "format_version": 1, "kind": "devex-clone-run", "id": "authority-fixture",
                "source_request": bound(&root.join("source-request.json")), "source_export": null,
                "initialized": bound(&target.join("initialized.json")),
                "source_environment": bound(&root.join("environment.json")),
                "target_environment": bound(&root.join("environment.json")),
                "copy_directory": root.join("copy"), "copy_stage": "source_to_seed", "build_bridges": [],
            }),
        );
        let value = json!({
            "format_version": 1, "kind": "devex-clone-target-authority",
            "initialized": bound(&target.join("initialized.json")),
            "request": bound(&root.join("request.json")),
            "initialized_files": bound(&root.join("initialized-files.json")),
            "files": tree::snapshot(&backend, &target, false).unwrap(),
            "generation": {"scope_id": "authority-fixture"},
            "remote_writes": 0, "target_ready": false, "clone_verified": false, "restore_qualified": false,
        });
        Self {
            backend,
            _directory: directory,
            run,
            target,
            value,
        }
    }

    fn validate(&self) -> crate::Result<()> {
        authority::validate(&self.backend, &self.run, &self.value)
    }
}

fn bound(path: &Path) -> Value {
    let digest = digest::file(path).unwrap();
    json!({"path": path, "bytes": digest.bytes, "sha256": digest.sha256})
}

fn write(path: &Path, value: &Value) {
    json_io::write_new(path, value).unwrap();
}

#[test]
fn authority_checks_actual_files_and_preserves_empty_directories_without_writes() {
    let fixture = Fixture::new();
    let before = tree::snapshot(&fixture.backend, fixture._directory.path(), false).unwrap();
    fixture.validate().unwrap();
    assert_eq!(
        tree::snapshot(&fixture.backend, fixture._directory.path(), false).unwrap(),
        before
    );
    assert!(
        fixture.value["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "empty-directory")
    );
}

#[test]
fn authority_rejects_qualifications_writes_and_coerced_boolean_values() {
    let fixture = Fixture::new();
    for (field, invalid) in [
        ("format_version", json!(true)),
        ("remote_writes", json!(true)),
        ("remote_writes", json!(1)),
        ("target_ready", json!(true)),
        ("clone_verified", json!(0)),
        ("restore_qualified", json!(null)),
    ] {
        let mut value = fixture.value.clone();
        value[field] = invalid;
        assert!(
            authority::validate(&fixture.backend, &fixture.run, &value).is_err(),
            "{field}"
        );
    }
    let mut value = fixture.value.clone();
    value["unknown"] = json!(false);
    assert!(authority::validate(&fixture.backend, &fixture.run, &value).is_err());
}

#[test]
fn authority_rejects_source_generation_and_complete_file_tree_drift() {
    let mut fixture = Fixture::new();
    fixture.value["generation"]["scope_id"] = json!("other-scope");
    assert!(fixture.validate().is_err());
    fixture.value["generation"]["scope_id"] = json!("authority-fixture");
    fixture.validate().unwrap();
    fs::remove_dir(fixture.target.join("empty-directory")).unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_rebound_request_and_foreign_lineage_paths() {
    let mut fixture = Fixture::new();
    let other = fixture._directory.path().join("other.json");
    write(&other, &json!({"structural_fixture": true}));
    fixture.value["request"] = bound(&other);
    assert!(fixture.validate().is_err());
    fixture.value["request"] = bound(&fixture._directory.path().join("request.json"));
    fixture.value["initialized_files"] = bound(&other);
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_modified_initialized_and_raw_target_request() {
    let fixture = Fixture::new();
    fs::write(fixture.target.join("request.json"), b"{\"changed\":true}").unwrap();
    assert!(fixture.validate().is_err());
    fs::write(
        fixture.target.join("initialized.json"),
        b"{\"changed\":true}",
    )
    .unwrap();
    assert!(fixture.validate().is_err());
}

#[test]
fn authority_rejects_missing_guard_before_a_controller_can_create_it() {
    let mut fixture = Fixture::new();
    fs::remove_file(fixture.target.join("copy-target.guard")).unwrap();
    fixture.value["files"] = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let before = fixture.value["files"].clone();
    assert!(
        fixture
            .validate()
            .unwrap_err()
            .to_string()
            .contains("规范控制互斥文件")
    );
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap(),
        before
    );
    assert!(!fixture.target.join("copy-target.guard").exists());
}

#[test]
fn authority_rejects_residual_lock_without_removing_its_contents() {
    let mut fixture = Fixture::new();
    let lock = fixture.target.join("initialize.lock");
    fs::create_dir(&lock).unwrap();
    fs::write(lock.join("unknown.json"), b"retain").unwrap();
    fixture.value["files"] = tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let before = fixture.value["files"].clone();
    assert!(
        fixture
            .validate()
            .unwrap_err()
            .to_string()
            .contains("初始化锁仍存在")
    );
    assert_eq!(
        tree::snapshot(&fixture.backend, &fixture.target, false).unwrap(),
        before
    );
    assert_eq!(fs::read(lock.join("unknown.json")).unwrap(), b"retain");
}
