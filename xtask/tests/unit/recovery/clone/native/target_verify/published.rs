use std::{collections::BTreeSet, fs, path::Path};

use serde_json::{Value, json};

use super::*;
use crate::recovery::clone::native::target_verify::tree as target_tree;

#[path = "../../../../../internal/target_verify_fixture.rs"]
mod fixture;

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn receipt(fixture: &fixture::Fixture, names: &mut BTreeSet<String>, stage: &str, value: &Value) {
    let name = format!("{stage}-{}.command.json", uuid::Uuid::new_v4().simple());
    write(&fixture.target.join(&name), value);
    assert!(names.insert(name));
}

fn command(command: Value, stdout: Value) -> Value {
    json!({"command":command,"returncode":0,"error_type":null,"stdout":stdout,"stderr":""})
}

fn observations(fixture: &fixture::Fixture) -> BTreeSet<String> {
    let mut names = BTreeSet::from(["request.json".into(), "prepare.json".into()]);
    for db in fixture.request["target"]["databases"].as_array().unwrap() {
        receipt(
            fixture,
            &mut names,
            &format!("mysql-{}", db["key"].as_str().unwrap()),
            &command(
                json!(["fixture-mysql"]),
                json!(format!("{}\n", db["server_uuid"].as_str().unwrap())),
            ),
        );
    }
    let pid = fixture.request["storage"]["redis"]["pid"].as_u64().unwrap();
    for _ in 0..2 {
        for index in 0..5 {
            let (args, stdout) = if index == 0 {
                (
                    json!(["/usr/bin/cat", format!("/proc/{pid}/stat")]),
                    json!(format!("{pid} (redis) {}", vec!["1"; 20].join(" "))),
                )
            } else {
                (
                    json!(["fixture-redis", format!("observation-{index}")]),
                    json!("fixed\n"),
                )
            };
            receipt(fixture, &mut names, "redis-kernel", &command(args, stdout));
        }
        let name = format!("storage-layout-{}.json", uuid::Uuid::new_v4().simple());
        write(&fixture.target.join(&name), &fixture.request["storage"]);
        names.insert(name);
    }
    object_observations(fixture, &mut names);
    names
}

fn object_observations(fixture: &fixture::Fixture, names: &mut BTreeSet<String>) {
    for (stage, operation) in [
        ("bucket-readiness", "head-bucket"),
        ("bucket-versioning", "get-bucket-versioning"),
        ("scoped-list", "list-objects-v2"),
    ] {
        for bucket in super::super::evidence_model::BUCKETS {
            let list = operation == "list-objects-v2";
            let response = if list {
                json!({"IsTruncated":false,"Contents":[]})
            } else {
                json!({})
            };
            receipt(
                fixture,
                names,
                stage,
                &json!({
                    "transport":"rust-s3-sigv4","operation":operation,"bucket":bucket,
                    "returncode":0,"error_type":null,"response":response,"key":null,"destination":null,
                    "remote_writes":0,"scope_id":fixture.request["target"]["scope_id"],
                    "endpoint":fixture.request["target"]["s3"]["endpoint"],
                    "configuration_sha256":digest::plan_hash(&fixture.request["target"]["s3"]).unwrap(),
                    "prefix":if list {json!(format!("{}/",fixture.request["target"]["scope_id"].as_str().unwrap()))} else {Value::Null},
                    "continuation":null
                }),
            );
        }
    }
}

fn complete() -> fixture::Fixture {
    let fixture = fixture::Fixture::new();
    let names = observations(&fixture);
    let files = target_tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    let prepared_files = files
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| names.contains(item["path"].as_str().unwrap()))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(prepared_files.len(), names.len());
    let prepared_path = fixture.local.join("prepared-files.json");
    let mut prepared = json_io::read(&prepared_path).unwrap();
    prepared["files"] = json!(prepared_files);
    write(&prepared_path, &prepared);
    refresh(&fixture);
    fixture
}

fn refresh(fixture: &fixture::Fixture) {
    let path = fixture.local.join("initialized-files.json");
    let mut initialized = json_io::read(&path).unwrap();
    initialized["predecessor"] =
        binding(&fixture.backend, &fixture.local.join("prepared-files.json")).unwrap();
    initialized["files"] = target_tree::snapshot(&fixture.backend, &fixture.target, false).unwrap();
    write(&path, &initialized);
}

#[test]
fn published_unlocked_history_requires_complete_double_tree_and_original_request() {
    let fixture = complete();
    let filename = fixture.target.join("initialized.json");
    let (initialized, request) = unlocked_history(&fixture.backend, &filename).unwrap();
    assert_eq!(request, fixture.request);
    assert_eq!(initialized, json_io::read(&filename).unwrap());
    assert_eq!(
        request_binding(&fixture.backend, &request).unwrap()["selected"]["scope_id"],
        request["target"]["scope_id"]
    );
}

#[test]
fn published_history_rejects_predecessor_and_target_tree_drift() {
    let fixture = complete();
    let path = fixture.local.join("prepared-files.json");
    let original = fs::read(&path).unwrap();
    let mut prepared = json_io::read(&path).unwrap();
    prepared["predecessor"] = json!({"unknown":"predecessor"});
    write(&path, &prepared);
    refresh(&fixture);
    let error = unlocked_history(&fixture.backend, &fixture.target.join("initialized.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("predecessor"), "{error}");
    fs::write(&path, original).unwrap();
    refresh(&fixture);
    fs::write(
        fixture.target.join("unknown-write"),
        b"preserve this evidence",
    )
    .unwrap();
    let error = unlocked_history(&fixture.backend, &fixture.target.join("initialized.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("文件树"), "{error}");
    assert_eq!(
        fs::read(fixture.target.join("unknown-write")).unwrap(),
        b"preserve this evidence"
    );
}

#[test]
fn published_history_rejects_unresolved_failure_without_mutating_evidence() {
    let fixture = complete();
    let path = fixture.target.join("failure-unrecognized.json");
    fs::write(&path, b"{}").unwrap();
    refresh(&fixture);
    let error = unlocked_history(&fixture.backend, &fixture.target.join("initialized.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("未解决"), "{error}");
    assert_eq!(fs::read(path).unwrap(), b"{}");
}

#[test]
fn published_history_never_accepts_an_existing_initialize_lock() {
    let parent = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&parent).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("published-unlocked-")
        .tempdir_in(parent)
        .unwrap();
    let target = temporary.path().join(".local-tests/target");
    fs::create_dir_all(&target).unwrap();
    for directory in [false, true] {
        let lock = target.join("initialize.lock");
        if directory {
            fs::create_dir(&lock).unwrap();
        } else {
            fs::write(&lock, b"owned elsewhere").unwrap();
        }
        let error = unlocked_history(temporary.path(), &target.join("initialized.json"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("initialize.lock"), "{error}");
        assert!(lock.exists());
        if directory {
            fs::remove_dir(lock).unwrap();
        } else {
            fs::remove_file(lock).unwrap();
        }
    }
}

#[test]
fn published_owned_history_requires_real_generation_and_preserves_unlocked_rejection() {
    let fixture = complete();
    let filename = fixture.target.join("initialized.json");
    let expected = unlocked_history(&fixture.backend, &filename).unwrap();
    let owner = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    owner.protects(&fixture.target).unwrap();
    let observed = owned_history(&fixture.backend, &filename, &owner);
    let error = unlocked_history(&fixture.backend, &filename)
        .unwrap_err()
        .to_string();
    assert!(error.contains("initialize.lock"), "{error}");
    assert!(fixture.target.join("initialize.lock").is_dir());
    assert_eq!(owner.finish(observed).unwrap(), expected);
    assert!(!fixture.target.join("initialize.lock").exists());
    assert_eq!(
        unlocked_history(&fixture.backend, &filename).unwrap(),
        expected
    );
}

#[test]
fn published_owned_history_rejects_another_targets_generation() {
    let fixture = complete();
    let other = fixture.local.join("other-target");
    fs::create_dir(&other).unwrap();
    let owner = guard::Generation::acquire(&fixture.backend, &other).unwrap();
    let error = owned_history(
        &fixture.backend,
        &fixture.target.join("initialized.json"),
        &owner,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("不属于当前目标目录"), "{error}");
    assert!(other.join("initialize.lock").is_dir());
    assert!(!fixture.target.join("initialize.lock").exists());
    owner.finish(Ok(())).unwrap();
}

#[test]
fn published_owned_history_rejects_unknown_lock_contents_without_cleanup() {
    let fixture = complete();
    let owner = guard::Generation::acquire(&fixture.backend, &fixture.target).unwrap();
    let marker = fixture.target.join("initialize.lock/owned-test-marker");
    fs::write(&marker, b"unknown lock write must survive rejection").unwrap();
    let error = owned_history(
        &fixture.backend,
        &fixture.target.join("initialized.json"),
        &owner,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("目录锁包含未知写入"), "{error}");
    assert_eq!(
        fs::read(&marker).unwrap(),
        b"unknown lock write must survive rejection"
    );
    // 仅删除本测试刚创建且已逐字节核对的 marker，再正常释放真实持有者。
    fs::remove_file(&marker).unwrap();
    owner.finish(Ok(())).unwrap();
    assert!(!fixture.target.join("initialize.lock").exists());
}
