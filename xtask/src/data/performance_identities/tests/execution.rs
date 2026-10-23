use super::{evidence, execution, ledger::Ledger, run_lock::RunLock};
use crate::Result;
use serde_json::json;
use std::fs;
#[path = "fixture.rs"]
mod fixture;
use fixture::Fixture;

fn execute(
    fixture: &mut Fixture,
    mode: &str,
    checkpoint: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<serde_json::Value> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execution::execute(
            &fixture.plan,
            &fixture.root(),
            mode,
            &uuid::Uuid::new_v4().to_string(),
            &mut fixture.context,
            checkpoint,
        ))
}
fn apply(fixture: &mut Fixture) {
    execute(fixture, "apply", &mut |_| Ok(())).unwrap();
}

#[test]
fn native_apply_then_verify_keeps_all_writes_and_exact_ordinary_sessions() {
    let mut fixture = Fixture::new();
    apply(&mut fixture);
    assert_eq!(fixture.ledger()["status"], "prepared");
    assert_eq!(fixture.ledger()["entries"].as_array().unwrap().len(), 622);
    assert_eq!(fixture.context.data.borrow().writes, 622);
    assert!(!fs::read_dir(fixture.root()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("verified-")
    }));
    let value = execute(&mut fixture, "verify", &mut |_| Ok(())).unwrap();
    assert_eq!(value["status"], "verified");
    let state = fixture.ledger();
    let receipt = evidence::parse(
        &fs::read(
            fixture
                .root()
                .join(state["verified_receipt"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["identities"].as_array().unwrap().len(), 200);
    assert_eq!(receipt["templates"].as_array().unwrap().len(), 10);
    assert_eq!(receipt["message_audience"].as_array().unwrap().len(), 10);
    assert!(
        !serde_json::to_string(&receipt)
            .unwrap()
            .contains("Fixture-User!123")
    );
    let data = fixture.context.data.borrow();
    assert_eq!(data.sessions, data.closed);
    assert_eq!(data.writes, 622);
    for secret in [
        "Fixture-User!123",
        "Fixture-Admin!123",
        "Fixture-Database!123",
        "fixture-secret",
    ] {
        assert!(
            !serde_json::to_string(&fixture.plan)
                .unwrap()
                .contains(secret)
        );
        assert!(!serde_json::to_string(&state).unwrap().contains(secret));
        assert!(!serde_json::to_string(&receipt).unwrap().contains(secret));
        assert!(!serde_json::to_string(&value).unwrap().contains(secret));
        for entry in fs::read_dir(fixture.root()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                assert!(
                    !String::from_utf8_lossy(&fs::read(entry.path()).unwrap()).contains(secret)
                );
            }
        }
    }
}
#[test]
fn confirmed_group_prefix_resumes_without_repeating_remote_writes() {
    let mut fixture = Fixture::new();
    // 11 次预检关闭，再在 system 组的首个改密会话关闭后终止。
    fixture.context.data.borrow_mut().fail_close_at = Some(12);
    assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.context.data.borrow().writes, 5);
    assert_eq!(fixture.ledger()["status"], "applying");
    fixture.context.data.borrow_mut().fail_close_at = None;
    apply(&mut fixture);
    assert_eq!(fixture.context.data.borrow().writes, 622);
}
#[test]
fn committed_unknown_write_marks_reconciliation_and_never_replays() {
    let mut fixture = Fixture::new();
    fixture.context.data.borrow_mut().fail_operation = Some("post_system_users".into());
    assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.ledger()["status"], "needs-reconciliation");
    let before = fixture.context.data.borrow().writes;
    assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
    assert!(execute(&mut fixture, "verify", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.context.data.borrow().writes, before);
}
#[test]
fn ownership_and_process_drift_fail_before_first_mutation() {
    for drift in [false, true] {
        let mut fixture = Fixture::new();
        if drift {
            fixture.context.data.borrow_mut().drift_inspection = Some(2);
        } else {
            fixture.context.data.borrow_mut().fail_inspection = Some(1);
        }
        assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
        assert_eq!(fixture.context.data.borrow().writes, 0);
        if !drift {
            assert_eq!(fixture.context.data.borrow().sessions, 0);
        }
    }
}
#[test]
fn invalid_activation_is_unknown_and_ordinary_authority_cannot_publish() {
    let mut fixture = Fixture::new();
    fixture.context.data.borrow_mut().bad_fragment = true;
    assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.ledger()["status"], "needs-reconciliation");
    let mut fixture = Fixture::new();
    apply(&mut fixture);
    fixture.context.data.borrow_mut().bad_ordinary = true;
    assert!(execute(&mut fixture, "verify", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.ledger()["status"], "verifying");
    fixture.context.data.borrow_mut().bad_ordinary = false;
    execute(&mut fixture, "verify", &mut |_| Ok(())).unwrap();
}
#[test]
fn template_department_mismatch_prevents_verification_publication() {
    let mut fixture = Fixture::new();
    apply(&mut fixture);
    fixture.context.data.borrow_mut().different_template = true;
    assert!(execute(&mut fixture, "verify", &mut |_| Ok(())).is_err());
    assert_eq!(fixture.ledger()["status"], "verifying");
}
#[test]
fn every_publication_boundary_recovers_without_remote_sessions() {
    for boundary in [
        "receipt-boundary",
        "ledger-boundary",
        "outer-result-boundary",
    ] {
        let mut fixture = Fixture::new();
        apply(&mut fixture);
        assert!(
            execute(&mut fixture, "verify", &mut |stage| if stage == boundary {
                Err("模拟崩溃".into())
            } else {
                Ok(())
            })
            .is_err()
        );
        let before = (
            fixture.context.data.borrow().sessions,
            fixture.context.data.borrow().inspections,
        );
        assert_eq!(
            execute(&mut fixture, "verify", &mut |_| Ok(())).unwrap()["status"],
            "verified"
        );
        assert_eq!(
            before,
            (
                fixture.context.data.borrow().sessions,
                fixture.context.data.borrow().inspections
            )
        );
    }
}
#[test]
fn resume_rechecks_template_and_rejects_receipt_tampering_or_another_root() {
    for case in ["template", "receipt", "root"] {
        let mut fixture = Fixture::new();
        apply(&mut fixture);
        assert!(execute(&mut fixture, "verify", &mut |_| Err("模拟崩溃".into())).is_err());
        let token = uuid::Uuid::new_v4().to_string();
        let mut ledger = Ledger::open(&fixture.plan, &fixture.root(), "verify", &token).unwrap();
        let (intent, _) = ledger.names();
        ledger.close().unwrap();
        let path = fixture.root().join(intent);
        let mut value = evidence::parse(&fs::read(&path).unwrap()).unwrap();
        match case {
            "template" => fs::write(
                value["receipt"]["templates"][0]["path"].as_str().unwrap(),
                "changed",
            )
            .unwrap(),
            "receipt" => {
                value["receipt"]["identities"][0]["user_id"] = json!("999999");
                fs::write(&path, evidence::encode(&value).unwrap()).unwrap();
            }
            _ => {
                value["identity_state"] = json!(fixture.directory.join("another-root"));
                fs::write(&path, evidence::encode(&value).unwrap()).unwrap();
            }
        }
        let before = fixture.context.data.borrow().sessions;
        assert!(execute(&mut fixture, "verify", &mut |_| Ok(())).is_err());
        assert_eq!(before, fixture.context.data.borrow().sessions);
    }
}
#[test]
fn unexpected_publication_blocks_both_operations_and_remains_untouched() {
    let mut fixture = Fixture::new();
    apply(&mut fixture);
    let path = fixture.root().join("verified-unexpected.json");
    fs::write(&path, "evidence").unwrap();
    for mode in ["apply", "verify"] {
        assert!(execute(&mut fixture, mode, &mut |_| Ok(())).is_err());
    }
    assert_eq!(fs::read_to_string(path).unwrap(), "evidence");
}
#[test]
fn lock_replacement_and_incomplete_release_remain_closed() {
    for boundary in ["before-claim", "claimed"] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.root()).unwrap();
        let lock = RunLock::acquire(
            &fixture.root(),
            &fixture.plan,
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(
            lock.release_with(&mut |stage, path| {
                if stage == boundary {
                    fs::rename(path, path.with_extension("displaced")).unwrap();
                    fs::write(path, "replacement").unwrap();
                }
                Ok(())
            })
            .is_err()
        );
        assert!(
            RunLock::acquire(
                &fixture.root(),
                &fixture.plan,
                &uuid::Uuid::new_v4().to_string()
            )
            .is_err()
        );
    }
}
#[test]
fn old_owner_preserves_new_lock_and_complete_release_can_reacquire() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root()).unwrap();
    let lock = RunLock::acquire(
        &fixture.root(),
        &fixture.plan,
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    lock.release_with(&mut |stage, _| {
        if stage == "claimed" {
            fs::write(fixture.root().join("lock"), "next-owner").unwrap();
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.root().join("lock")).unwrap(),
        "next-owner"
    );
    fs::remove_file(fixture.root().join("lock")).unwrap();
    RunLock::acquire(
        &fixture.root(),
        &fixture.plan,
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap()
    .release()
    .unwrap();
}

#[test]
fn private_configuration_drift_and_connection_target_changes_fail_closed() {
    let fixture = Fixture::new();
    let private = super::plan::load(&fixture.plan, &crate::workspace::root_dir()).unwrap();
    let connection = &private["database"]["targets"][0]["connection"];
    let mut target = connection.clone();
    target["key"] = json!("shared");
    target["kind"] = json!("mysql");
    target["mode"] = json!("shared");
    let table = json!({"scope_id":private["scope_id"],"cors":{"allow_origins":["http://127.0.0.1:4199"]},"database":{"primary":private["database"]["control"]},"tenant_data":{"targets":[target]}});
    super::inspection::bindings(&private, &table).unwrap();
    for (key, value) in [
        ("database", json!("other")),
        ("username", json!("other")),
        ("password", json!("changed-secret")),
        ("port", json!(3307)),
        ("host", json!("localhost")),
        ("tls_mode", json!("disabled")),
        ("mode", json!("dedicated")),
        ("kind", json!("control")),
        ("key", json!("other")),
    ] {
        let mut changed = table.clone();
        changed["tenant_data"]["targets"][0][key] = value;
        let failure = super::inspection::bindings(&private, &changed)
            .unwrap_err()
            .to_string();
        assert!(!failure.contains("changed-secret"));
    }
    let path = fixture.plan["configuration"]["path"].as_str().unwrap();
    fs::write(path, "changed-secret").unwrap();
    let failure = super::plan::load(&fixture.plan, &crate::workspace::root_dir())
        .unwrap_err()
        .to_string();
    assert!(!failure.contains("changed-secret"));
}

#[test]
fn quota_requires_active_exact_limits_placement_and_current_capacity() {
    let fixture = Fixture::new();
    let environment = &fixture.plan["environment"];
    let group = &fixture.plan["groups"][1];
    let row = json!({"tenant_id":group["tenant_id"],"status":"enabled","max_users":1000,"users":1,"roles":1,"max_roles":10,"target_key":"shared","placement_state":"active"});
    super::inspection::quota(&row, group, environment, "apply").unwrap();
    for (key, value) in [
        ("max_users", json!(1001)),
        ("users", json!(891)),
        ("target_key", json!("other")),
        ("placement_state", json!("maintenance")),
        ("status", json!("disabled")),
        ("roles", json!(10)),
    ] {
        let mut changed = row.clone();
        changed[key] = value;
        assert!(super::inspection::quota(&changed, group, environment, "apply").is_err());
    }
    let mut verified = row;
    verified["users"] = json!(900);
    verified["roles"] = json!(10);
    super::inspection::quota(&verified, group, environment, "verify").unwrap();
}

#[test]
fn another_plans_started_ledger_is_preserved_without_any_state_rewrite() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.root()).unwrap();
    let path = fixture.root().join("ledger.json");
    let value = json!({
        "format_version":1,"plan_sha256":"a".repeat(64),"status":"applying",
        "roles":{},"users":{},"entries":[{"phase":"started"}]
    });
    evidence::new(&path, &value).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(execute(&mut fixture, "apply", &mut |_| Ok(())).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(fixture.context.data.borrow().sessions, 0);
}
