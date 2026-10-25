use crate::{
    recovery::clone::native::{
        cache,
        resources::common,
        target_verify::{fresh, tree},
    },
    target_verify_fixture::{Fixture, bound, snapshot, write},
};
use serde_json::{Value, json};
use std::fs;

#[test]
fn native_status_reads_complete_initialized_history_and_blocks_unknown_tree_without_writes() {
    let f = Fixture::for_status();
    let before = snapshot(&f.local);
    let result = fresh::observe(&f.backend, &f.local).unwrap();
    assert_eq!(result["status"], "fresh_target_initialized", "{result}");
    assert_eq!(result["next_action"], "verify");
    assert_eq!(result["restore_qualified"], false);
    assert_eq!(snapshot(&f.local), before);
    fs::write(f.target.join("unknown-write.txt"), "unowned").unwrap();
    let before = snapshot(&f.local);
    let result = fresh::observe(&f.backend, &f.local).unwrap();
    assert_eq!(result["next_action"], Value::Null);
    assert_eq!(result["evidence_valid"], false);
    assert_eq!(snapshot(&f.local), before);
}

#[test]
fn native_status_interrupted_registration_and_invalid_request_keep_all_local_evidence() {
    let f = Fixture::for_status();
    let storage = f.backend.join(".local-tests/status-storage");
    // 仅删除本测试独占的阶段产物，模拟尚未开始 prepare 的已登记状态。
    fs::remove_dir_all(&f.target).unwrap();
    for file in ["prepared-files.json", "initialized-files.json"] {
        fs::remove_file(f.local.join(file)).unwrap();
    }
    let before = snapshot(&f.local);
    let result = fresh::observe(&f.backend, &f.local).unwrap();
    assert_eq!(result["next_action"], "resume-prepare");
    assert_eq!(snapshot(&f.local), before);
    let source = fs::read(storage.join("state.json")).unwrap();
    assert_eq!(
        cache::binding::load(&f.backend, &storage, None).unwrap(),
        Value::Null
    );
    assert_eq!(fs::read(storage.join("state.json")).unwrap(), source);
    fs::write(f.local.join("mysql"), "changed-tool").unwrap();
    let before = snapshot(&f.local);
    let result = fresh::observe(&f.backend, &f.local).unwrap();
    assert_eq!(result["blocking_reason"], "固定请求未达到可执行条件");
    assert_eq!(result["next_action"], Value::Null);
    assert_eq!(snapshot(&f.local), before);
    assert!(!f.local.join("fresh-target.guard").exists());
}

#[test]
fn native_status_observes_actual_live_controller_and_reused_identity_without_recovery_permission() {
    for matches in [true, false] {
        let f = Fixture::for_status();
        let storage = f.backend.join(".local-tests/status-storage");
        fs::remove_dir_all(&f.target).unwrap();
        let identity = common::process_identity(std::process::id())
            .unwrap()
            .unwrap();
        let lock = storage.join("run.lock");
        fs::create_dir(&lock).unwrap();
        let directory = crate::recovery::clone::native::lifecycle::manifest::path_text(
            &storage,
            "状态夹具控制器目录",
        )
        .unwrap();
        let mut owner = json!({"format_version":1,"identity":identity,"directory":directory,
            "manifest_sha256":bound(&storage.join("manifest.json"))["sha256"]});
        if !matches {
            owner["identity"]["started"] = json!("1");
        }
        crate::recovery::clone::native::lifecycle::identity::validate(&owner, &storage)
            .expect("控制器夹具必须满足正式身份与路径格式");
        write(&lock.join("owner.json"), &owner);
        let before = snapshot(&f.backend.join(".local-tests"));
        let result = fresh::observe(&f.backend, &f.local).unwrap();
        assert_eq!(result["pending_stage"], "controller", "{result}");
        assert_eq!(result["controller"]["process_matches"], matches);
        assert_eq!(result["controller"]["process_missing"], false);
        assert_eq!(result["next_action"], Value::Null);
        assert_eq!(snapshot(&f.backend.join(".local-tests")), before);
    }
}

#[test]
fn native_status_validates_prepared_snapshot_and_publication_resume_separately() {
    let f = Fixture::for_status();
    let prepared: Value =
        serde_json::from_slice(&fs::read(f.local.join("prepared-files.json")).unwrap()).unwrap();
    let keep = prepared["files"].as_array().unwrap();
    let current = tree::snapshot(&f.backend, &f.target, false).unwrap();
    for item in current.as_array().unwrap().iter().rev() {
        if keep.iter().any(|old| old["path"] == item["path"]) {
            continue;
        }
        let path = f.target.join(item["path"].as_str().unwrap());
        if item["type"] == "directory" {
            fs::remove_dir(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
    let before = snapshot(&f.local);
    let result = fresh::observe(&f.backend, &f.local).unwrap();
    assert_eq!(result["next_action"], "initialize", "{result}");
    assert_eq!(snapshot(&f.local), before);
    fs::remove_file(f.local.join("prepared-files.json")).unwrap();
    let before = snapshot(&f.local);
    assert_eq!(
        fresh::observe(&f.backend, &f.local).unwrap()["next_action"],
        "resume-prepare"
    );
    assert_eq!(snapshot(&f.local), before);
    fs::write(f.target.join("prepare.json"), "{}").unwrap();
    let before = snapshot(&f.local);
    assert_eq!(
        fresh::observe(&f.backend, &f.local).unwrap()["evidence_valid"],
        false
    );
    assert_eq!(snapshot(&f.local), before);
}

#[test]
fn native_status_requires_new_workspace_after_bound_preflight_reconciliation() {
    for valid in [true, false] {
        let f = Fixture::for_status();
        let report_dir = f.target.join("reset-state");
        for entry in fs::read_dir(&report_dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".report.json")
            {
                fs::remove_file(path).unwrap();
            }
        }
        let failure = f.target.join("failure.json");
        let report = report_dir.join("preflight.report.json");
        write(&failure, &json!({"status":"needs_reconciliation"}));
        write(
            &report,
            &json!({"status":"failed","failed_phase":"preflight"}),
        );
        let short = |path: &std::path::Path| {
            let mut value = bound(path);
            value.as_object_mut().unwrap().remove("path");
            value
        };
        let path = f.target.join("reconciliation-completed.json");
        let value = json!({"status":"preflight_failure_reconciled",
            "before":{"databases":[{"exists":true}],"objects":{},"redis":{}},
            "after":{"databases":[{"exists":false}],"objects":{},"redis":{}},
            "failure":short(&failure),"reset_report":short(&report),"automatic_retry":!valid,"restore_qualified":false});
        write(&path, &value);
        let before = snapshot(&f.local);
        let result = fresh::observe(&f.backend, &f.local).unwrap();
        if valid {
            assert_eq!(result["status"], "fresh_target_preflight_reconciled");
            assert_eq!(result["next_action"], "prepare-new-workspace");
            assert_eq!(result["reconciliation"], bound(&path));
        } else {
            assert_eq!(result["next_action"], Value::Null);
        }
        assert_eq!(snapshot(&f.local), before);
    }
}
