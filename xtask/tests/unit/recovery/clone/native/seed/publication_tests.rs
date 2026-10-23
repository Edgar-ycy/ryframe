use super::{Registration, fixture::Fixture, history};
use crate::recovery::clone::native::{json as document, lifecycle::state, resources::common};
use serde_json::{Value, json};
use std::fs;

#[test]
fn register_preserves_original_api_and_does_not_authorize_worker_or_create_ledger() {
    let mut fixture = Fixture::new();
    let before = common::binding(&fixture.old.join("api.json")).unwrap();
    let args = fixture.begin_registration();
    let result = fixture.protocol("register", args.clone()).unwrap();
    assert_eq!(
        result,
        json!({"status":"seed_runtime_registered","registration":common::binding(&fixture.run.join("seed-runtime.json")).unwrap(),
        "worker_authorized":false,"remote_writes":0,"restore_qualified":false})
    );
    assert_eq!(
        before,
        common::binding(&fixture.old.join("api.json")).unwrap()
    );
    assert!(!fixture.run.join("seed-runtime").exists());
    let raw = fs::read(fixture.run.join("seed-runtime.json")).unwrap();
    assert_eq!(fixture.protocol("register", args).unwrap(), result);
    assert_eq!(
        raw,
        fs::read(fixture.run.join("seed-runtime.json")).unwrap()
    );
    fixture.finish_registration(&result);
    assert_eq!(
        Registration::load(&fixture.backend, &fixture.run, false)
            .unwrap()
            .value,
        fixture.request
    );
    assert_eq!(
        fixture.protocol("inputs", json!({})).unwrap()["identity_plan"],
        fixture.identities.plan
    );
}

#[test]
fn registration_requires_current_controller_attempt_owner_and_target_lock() {
    for field in [
        "attempt",
        "controller",
        "target_lock_inode",
        "owner",
        "state",
        "lock",
    ] {
        let mut fixture = Fixture::new();
        let mut args = fixture.begin_registration();
        match field {
            "attempt" => args[field] = json!(999),
            "controller" => {
                args[field] = common::binding(&fixture.run.join("historical.json")).unwrap()
            }
            "target_lock_inode" => args[field] = json!(0),
            "owner" => {
                let path = fixture.run.join("run.lock/owner.json");
                let mut owner = document::read(&path).unwrap();
                owner["identity"]["started"] = json!("1");
                document::write_replace(&path, &owner).unwrap();
                // 恢复 owner 后才释放，不能用错误 owner 清理其他人的锁。
                let original = fixture.owner.as_ref().unwrap().owner.clone();
                assert!(fixture.protocol("register", args.clone()).is_err());
                document::write_replace(&path, &original).unwrap();
            }
            "state" => {
                let mut value = state::load(&fixture.run, true).unwrap();
                value["attempts"]
                    .as_array_mut()
                    .unwrap()
                    .last_mut()
                    .unwrap()["mode"] = json!("prepare");
                document::write_replace(&fixture.run.join("state.json"), &value).unwrap();
            }
            "lock" => fs::remove_dir(fixture.run.join("target/initialize.lock")).unwrap(),
            _ => unreachable!(),
        }
        if field != "owner" {
            assert!(fixture.protocol("register", args).is_err(), "{field}");
        }
        assert!(!fixture.run.join("seed-runtime.json").exists());
    }
}

#[test]
fn register_rejects_partial_or_conflicting_existing_publication_without_overwriting() {
    for bytes in [
        b"{".as_slice(),
        b"{\"different\":true}",
        b"{\"duplicate\":1,\"duplicate\":2}",
    ] {
        let mut fixture = Fixture::new();
        let args = fixture.begin_registration();
        let path = fixture.run.join("seed-runtime.json");
        fs::write(&path, bytes).unwrap();
        assert!(fixture.protocol("register", args).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn later_failed_or_running_copy_and_post_verify_cannot_fall_back_to_old_success() {
    for (stage, mode) in [
        ("copy", "reconcile"),
        ("post-copy", "verify"),
        ("post-copy", "amend"),
    ] {
        for failed in [false, true] {
            let fixture = Fixture::new();
            let (number, _) = state::begin(&fixture.run, stage, mode, &json!({})).unwrap();
            if failed {
                state::finish(&fixture.run, number, None, Some("ValueError")).unwrap();
            }
            assert!(
                history::validate(&fixture.backend, &fixture.run, &fixture.request).is_err(),
                "{stage}/{mode}/{failed}"
            );
            assert!(!fixture.run.join("seed-runtime.json").exists());
        }
    }
}

#[test]
fn history_checks_exact_run_active_post_verify_node_and_raw_files() {
    let fixture = Fixture::new();
    fixture.file(
        "current-business-image",
        &json!({"identity_count":200,"outbox_delivered":123}),
    );
    assert!(history::validate(&fixture.backend, &fixture.run, &fixture.request).is_ok());
    for field in [
        "run_manifest",
        "post_copy",
        "post_verify",
        "node",
        "format_version",
        "kind",
    ] {
        let mut request = fixture.request.clone();
        request[field] = Value::Null;
        assert!(
            history::validate(&fixture.backend, &fixture.run, &request).is_err(),
            "{field}"
        );
    }
    fs::write(fixture.run.join("historical.json"), "{\"drift\":true}").unwrap();
    assert!(history::validate(&fixture.backend, &fixture.run, &fixture.request).is_err());
}

#[test]
fn private_file_or_bound_runtime_drift_never_publishes_registration() {
    for target in [
        "private.json",
        "post-copy/runtime/runtime.json",
        "post-copy/runtime/api.json",
        "identity-plan.json",
    ] {
        let mut fixture = Fixture::new();
        let args = fixture.begin_registration();
        fs::write(
            fixture.run.join(target),
            "{\"password\":\"untrusted-secret\"}",
        )
        .unwrap();
        assert!(fixture.protocol("register", args).is_err(), "{target}");
        assert!(!fixture.run.join("seed-runtime.json").exists());
    }
}

#[test]
fn unknown_duplicate_oversize_or_extra_protocol_fields_fail_before_writes() {
    let fixture = Fixture::new();
    for bytes in [
        vec![],
        vec![b' '; 65537],
        b"{\"operation\":1,\"operation\":2}".to_vec(),
    ] {
        assert!(super::protocol::execute(&fixture.backend, &bytes).is_err());
    }
    assert!(fixture.protocol("arm-input", json!({})).is_err());
    assert!(
        fixture
            .protocol("registration", json!({"cleanup":false,"password":"secret"}))
            .is_err()
    );
    assert!(!fixture.run.join("seed-runtime.json").exists());
}
