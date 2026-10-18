use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::{
    recovery::monitoring::{observe, publish},
    workspace::root_dir,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = root_dir()
            .join(".local-tests")
            .join(format!("monitor-publish-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn event() -> (Value, Value) {
    let binding =
        json!({"scope_id": "restore-scope", "endpoints": {"prometheus": "http://127.0.0.1:29090"}});
    let event = json!({
        "received_at": "2026-09-12T00:00:01Z", "remote": "127.0.0.1",
        "payload": {
            "receiver": "ryframe-local-webhook", "status": "firing",
            "alerts": [{"status": "firing", "labels": {
                "alertname": "RyFrameMonitoringDeliveryProbe", "service": "ryframe",
                "scope_id": "restore-scope", "probe_id": "a".repeat(32),
            }, "annotations": {"summary": "RyFrame 隔离监控投递验收"},
            "startsAt": "2026-09-12T00:00:00Z", "endsAt": "2026-09-12T00:10:00Z",
            "generatorURL": "http://127.0.0.1:29090", "fingerprint": "fixture"}],
        },
    });
    (binding, event)
}

#[test]
fn exact_probe_accepts_only_current_scope_receiver_identity_and_status() {
    let (binding, valid) = event();
    let check = |value: &Value| {
        observe::probe_event(value, &binding, &"a".repeat(32), "2026-09-12T00:00:00Z")
    };
    assert_eq!(check(&valid).unwrap(), Some("firing"));
    for (pointer, bad) in [
        ("/remote", json!("192.0.2.1")),
        ("/payload/receiver", json!("external")),
        ("/payload/status", json!("unknown")),
        ("/payload/alerts/0/status", json!("resolved")),
        ("/payload/alerts/0/labels/probe_id", json!("b".repeat(32))),
        ("/payload/alerts/0/labels/scope_id", json!("other")),
        ("/payload/alerts/0/startsAt", json!("2026-09-11T00:00:00Z")),
        (
            "/payload/alerts/0/generatorURL",
            json!("http://127.0.0.1:29091"),
        ),
    ] {
        let mut value = valid.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(check(&value).is_err(), "{pointer}");
    }
    for extra in [
        json!({"labels": {"alertname": "OtherAlert"}}),
        valid["payload"]["alerts"][0].clone(),
    ] {
        let mut value = valid.clone();
        value["payload"]["alerts"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        assert!(check(&value).is_err());
    }
    let mut resolved = valid;
    resolved["payload"]["status"] = json!("resolved");
    resolved["payload"]["alerts"][0]["status"] = json!("resolved");
    assert_eq!(check(&resolved).unwrap(), Some("resolved"));
    assert_eq!(check(&json!({"payload": {"alerts": []}})).unwrap(), None);
}

#[test]
fn terminal_success_is_create_only_and_removes_pending_at_commit_point() {
    let fixture = Fixture::new();
    let value =
        json!({"format_version": 1, "kind": "restore-monitoring-result", "status": "passed"});
    publish::terminal(&fixture.0, &value).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(fixture.0.join("result.json")).unwrap()).unwrap(),
        value
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    assert!(
        publish::terminal(&fixture.0, &value)
            .unwrap_err()
            .to_string()
            .contains("拒绝覆盖")
    );
}

#[test]
fn failed_pending_cleanup_preserves_unknown_publication_and_blocks_replay() {
    let fixture = Fixture::new();
    let value = json!({"status": "passed"});
    let result =
        publish::terminal_with_cleanup(
            &fixture.0,
            &value,
            |_| Err("fixture unlink failure".into()),
        );
    assert!(result.unwrap_err().to_string().contains("fixture"));
    assert!(fixture.0.join("result.json").is_file());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    assert!(
        publish::terminal(&fixture.0, &value)
            .unwrap_err()
            .to_string()
            .contains("pending")
    );
}
