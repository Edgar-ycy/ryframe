use crate::recovery::clone::native::{
    json as document,
    resources::common,
    target_verify::fresh::registration::{Registration, Workspace},
};
use crate::{
    target_initialize_restart_fixture as services,
    target_verify_fixture::{Fixture, snapshot},
};
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn registered_window_holds_the_real_run_owner_and_rejects_input_drift_without_replay() {
    service_windows();
    let fixture = Fixture::for_status();
    let registration = Registration::load(&fixture.backend, &fixture.local).unwrap();
    let registered = document::read(&fixture.local.join("registration.json")).unwrap();
    let storage = PathBuf::from(registered["storage_run"]["path"].as_str().unwrap());
    let before = common::binding(&storage.join("state.json")).unwrap();
    let value = registration
        .execute(|active| {
            assert_eq!(active, Some(storage.as_path()));
            let owner = document::read(&storage.join("run.lock/owner.json"))?;
            assert_eq!(owner["identity"]["pid"], std::process::id());
            assert_eq!(common::binding(&storage.join("state.json"))?, before);
            Ok(json!({"remote_writes":0}))
        })
        .unwrap();
    assert_eq!(value["remote_writes"], 0);
    assert!(!storage.join("run.lock").exists());
    let path = fixture.local.join("registration.json");
    let raw = fs::read(&path).unwrap();
    let error = registration
        .execute::<Value>(|_| {
            fs::write(&path, b"{}")?;
            Err("original-observation-failure".into())
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("original-observation-failure"));
    assert!(error.contains("登记"));
    fs::write(&path, raw).unwrap();
    assert!(!storage.join("run.lock").exists());
    assert_eq!(
        common::binding(&storage.join("state.json")).unwrap(),
        before
    );
}

fn service_windows() {
    let fixture = Fixture::for_status();
    for phase in ["initial", "restart", "closed", "unpublished-restart"] {
        let storage = fixture
            .backend
            .join(format!(".local-tests/registration-services-{phase}"));
        services::initial(&fixture, &storage);
        let path = fixture.local.join("registration.json");
        let mut registered = document::read(&path).unwrap();
        registered["storage_run"] = json!({"path":storage,
            "manifest":common::binding(&storage.join("manifest.json")).unwrap(),
            "state":common::binding(&storage.join("state.json")).unwrap()});
        fs::write(&path, serde_json::to_vec(&registered).unwrap()).unwrap();
        match phase {
            "initial" => (),
            "closed" => {
                services::close(&storage);
            }
            _ => {
                services::restart(&fixture, &storage);
                if phase == "unpublished-restart" {
                    fs::remove_file(storage.join("lifecycle-0005/restart.json")).unwrap();
                }
            }
        }
        let before = common::binding(&storage.join("state.json")).unwrap();
        let target_before = snapshot(&fixture.target);
        let mut called = false;
        let result =
            Registration::load(&fixture.backend, &fixture.local).and_then(|registration| {
                registration.execute(|active| {
                    called = true;
                    assert_eq!(
                        active,
                        if phase == "initial" {
                            None
                        } else {
                            Some(storage.as_path())
                        }
                    );
                    let owner = document::read(&storage.join("run.lock/owner.json"))?;
                    assert_eq!(owner["identity"]["pid"], std::process::id());
                    Ok(json!({"remote_writes":0}))
                })
            });
        let available = matches!(phase, "initial" | "restart");
        assert_eq!(result.is_ok(), available, "{phase}: {result:?}");
        assert_eq!(called, available, "{phase}");
        assert!(!storage.join("run.lock").exists());
        assert_eq!(
            common::binding(&storage.join("state.json")).unwrap(),
            before
        );
        assert_eq!(snapshot(&fixture.target), target_before);
    }
}

#[test]
fn workspace_guard_refuses_replacement_and_never_cleans_another_owner_lock() {
    let fixture = Fixture::for_status();
    let workspace = fixture.local.join("prepare-workspace");
    fs::create_dir(&workspace).unwrap();
    let guard = Workspace::claim(&fixture.backend, &workspace).unwrap();
    assert!(Workspace::claim(&fixture.backend, &workspace).is_err());
    guard.finish(Ok(())).unwrap();
    let registration = Registration::load(&fixture.backend, &fixture.local).unwrap();
    let registered = document::read(&fixture.local.join("registration.json")).unwrap();
    let storage = PathBuf::from(registered["storage_run"]["path"].as_str().unwrap());
    fs::create_dir(storage.join("run.lock")).unwrap();
    fs::write(storage.join("run.lock/foreign.bin"), b"unowned").unwrap();
    let mut called = false;
    let result = registration.execute::<Value>(|_| {
        called = true;
        Ok(Value::Null)
    });
    assert!(result.is_err());
    assert!(!called);
    assert_eq!(
        fs::read(storage.join("run.lock/foreign.bin")).unwrap(),
        b"unowned"
    );
}
