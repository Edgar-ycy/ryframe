use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::{
    recovery::fixture_control::source_pair::publish as publish_source_pair, workspace::root_dir,
};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    directory: PathBuf,
    backend: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "xtask-fixture-native-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let backend = directory.join("fixture/backend");
        fs::create_dir_all(&backend).unwrap();
        fs::create_dir_all(backend.join(".local-tests/reference-fixture")).unwrap();
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .arg(&backend)
            .status()
            .unwrap();
        assert!(status.success());
        Self { directory, backend }
    }

    fn write_fixture(&self) {
        write_json(
            &self.directory.join("fixture/fixture.json"),
            &fixture_receipt(&self.backend, &self.directory.join("fixture/frontend")),
        );
    }

    fn source_pair(&self) -> PathBuf {
        self.backend
            .join(".local-tests/reference-fixture/source-pair.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

#[test]
fn source_pair_publish_binds_fixture_and_is_create_only() {
    let fixture = Fixture::new();
    fixture.write_fixture();
    let output = fixture.source_pair();
    let receipt = publish_source_pair(&fixture.backend, &output).unwrap();
    assert_eq!(receipt["kind"], "reference-fixture-source-pair");
    assert_eq!(receipt["format_version"], 1);
    assert_eq!(
        receipt["fixture_receipt"]["path"],
        receipt_path_text(&fixture.directory.join("fixture/fixture.json"))
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&output).unwrap()).unwrap(),
        receipt
    );

    let error = publish_source_pair(&fixture.backend, &output).unwrap_err();
    assert!(error.to_string().contains("已存在"));
}

#[test]
fn source_pair_rejects_missing_unknown_duplicate_and_drifted_fields() {
    let fixture = Fixture::new();
    let receipt = fixture_receipt(
        &fixture.backend,
        &fixture.directory.join("fixture/frontend"),
    );
    let path = fixture.directory.join("fixture/fixture.json");

    let mut missing = receipt.clone();
    missing.as_object_mut().unwrap().remove("sources");
    write_json(&path, &missing);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let mut unknown = fixture_receipt(
        &fixture.backend,
        &fixture.directory.join("fixture/frontend"),
    );
    unknown["unexpected"] = json!(true);
    write_json(&path, &unknown);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let duplicate = serde_json::to_string(&receipt).unwrap();
    fs::write(&path, duplicate.replacen('{', "{\"format_version\":1,", 1)).unwrap();
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());

    let mut drift = receipt;
    drift["sources"]["backend"]["patch_sha256"] = json!("A".repeat(64));
    write_json(&path, &drift);
    assert!(publish_source_pair(&fixture.backend, &fixture.source_pair()).is_err());
}

#[test]
fn source_pair_rejects_out_of_bound_and_linked_outputs() {
    let fixture = Fixture::new();
    fixture.write_fixture();
    let outside = fixture.directory.join("fixture/source-pair.json");
    assert!(publish_source_pair(&fixture.backend, &outside).is_err());

    let allowed = fixture.backend.join(".local-tests/reference-fixture");
    let linked = fixture.backend.join(".local-tests/linked");
    create_directory_link(&allowed, &linked);
    let linked_output = linked.join("source-pair.json");
    assert!(publish_source_pair(&fixture.backend, &linked_output).is_err());
    remove_directory_link(&linked);
}

fn fixture_receipt(backend: &Path, frontend: &Path) -> Value {
    json!({
        "format_version": 1,
        "fixture": "business",
        "status": "ready",
        "fixture_sha256": "a".repeat(64),
        "sources": {
            "backend": clean_snapshot("b"),
            "frontend": clean_snapshot("c")
        },
        "paths": {
            "backend": path_text(backend),
            "frontend": path_text(frontend)
        },
        "generated": {
            "backend": clean_snapshot("b"),
            "frontend": clean_snapshot("c")
        }
    })
}

fn clean_snapshot(head: &str) -> Value {
    json!({
        "head": head.repeat(40),
        "patch_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "files": []
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .unwrap();
}

fn path_text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

fn receipt_path_text(path: &Path) -> String {
    #[cfg(windows)]
    {
        path_text(path).replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        path_text(path)
    }
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "New-Item -ItemType Junction -Path $env:RYFRAME_FIXTURE_LINK -Target $env:RYFRAME_FIXTURE_TARGET | Out-Null",
        ])
        .env("RYFRAME_FIXTURE_LINK", link)
        .env("RYFRAME_FIXTURE_TARGET", target)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    fs::remove_file(link).unwrap();
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    fs::remove_dir(link).unwrap();
}
