use serde_json::{Value, json};
use std::{fs, path::PathBuf};

use super::{common, document, history, model_tests as data};

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
    action: Value,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend.join(".local-tests").join(format!(
            "department-history-中文 空格-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(run.join("seed-runtime/departments/tenant-01")).unwrap();
        document::write_new(
            &run.join("seed-runtime/department-plan.json"),
            &json!({"fixture":"plan"}),
        )
        .unwrap();
        Self {
            backend,
            run,
            action: data::action(),
        }
    }
    fn attempt(&self, number: u32) -> PathBuf {
        let path = self
            .run
            .join(format!("seed-runtime/departments/tenant-01/{number:04}"));
        fs::create_dir(&path).unwrap();
        document::write_new(&path.join("before.json"), &data::observed(false)).unwrap();
        document::write_new(&path.join("intent.json"),&json!({"plan":common::binding(&self.run.join("seed-runtime/department-plan.json")).unwrap(),
            "target":self.action["target"],"before":common::binding(&path.join("before.json")).unwrap(),"body":self.action["body"],
            "operation":"post_system_depts","idempotency_key":"12345678-1234-4234-8234-123456789012","automatic_retry":false})).unwrap();
        path
    }
    fn reconcile(&self, attempt: &std::path::Path, number: u32, after: bool) -> Value {
        let observed = attempt.join(format!("observed-{number:04}.json"));
        document::write_new(&observed, &data::observed(after)).unwrap();
        let path = attempt.join(format!("reconcile-{number:04}.json"));
        document::write_new(&path,&json!({"intent":common::binding(&attempt.join("intent.json")).unwrap(),
            "observed":common::binding(&observed).unwrap(),"state":if after {"after"} else {"before"}})).unwrap();
        common::binding(&path).unwrap()
    }
    fn confirm(&self, attempt: &std::path::Path, response: bool) {
        let reconcile = self.reconcile(attempt, 2, true);
        let mut evidence = json!({"intent":common::binding(&attempt.join("intent.json")).unwrap(),"before":common::binding(&attempt.join("before.json")).unwrap(),
            "observed":common::binding(&attempt.join("observed-0002.json")).unwrap()});
        if response {
            document::write_new(&attempt.join("api-response.json"),&json!({"response":{"code":200,"data":data::row()},"authorization":data::auth(true)})).unwrap();
            evidence["api-response"] = common::binding(&attempt.join("api-response.json")).unwrap();
        } else {
            evidence["reconcile"] = reconcile;
        }
        document::write_new(&attempt.join("confirmed.json"),&json!({"plan":common::binding(&self.run.join("seed-runtime/department-plan.json")).unwrap(),
            "target":self.action["target"],"observed":data::observed(true),"api_response_proven":response,"post_attempted":true,"evidence":evidence})).unwrap();
    }
    fn history(&self, attempts: &[PathBuf]) -> crate::Result<Value> {
        history::action_history(
            &self.backend,
            &self.run,
            &self.action,
            &data::target(),
            attempts,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn unknown_write_cannot_replay_and_only_explicit_before_reconciliation_allows_same_key_retry() {
    let fixture = Fixture::new();
    let first = fixture.attempt(1);
    assert!(fixture.history(std::slice::from_ref(&first)).is_err());
    fixture.reconcile(&first, 1, false);
    let second = fixture.attempt(2);
    fixture.confirm(&second, false);
    let attempts = vec![first.clone(), second.clone()];
    assert_eq!(
        fixture.history(&attempts).unwrap(),
        common::binding(&second.join("confirmed.json")).unwrap()
    );
    let mut intent = document::read(&second.join("intent.json")).unwrap();
    intent["idempotency_key"] = json!("87654321-4321-4321-8321-cba987654321");
    fs::write(
        second.join("intent.json"),
        serde_json::to_vec(&intent).unwrap(),
    )
    .unwrap();
    assert!(fixture.history(&attempts).is_err());
    fs::remove_file(first.join("reconcile-0001.json")).unwrap();
    assert!(fixture.history(&attempts).is_err());
}

#[test]
fn response_and_unknown_commit_confirmation_require_bound_complete_images() {
    for response in [false, true] {
        let fixture = Fixture::new();
        let attempt = fixture.attempt(1);
        fixture.confirm(&attempt, response);
        let files = fs::read_dir(&attempt).unwrap().count();
        let before = fs::read(attempt.join("confirmed.json")).unwrap();
        assert!(fixture.history(std::slice::from_ref(&attempt)).is_ok());
        assert_eq!(fs::read(attempt.join("confirmed.json")).unwrap(), before);
        assert_eq!(fs::read_dir(&attempt).unwrap().count(), files);
        let mut value = document::read(&attempt.join("confirmed.json")).unwrap();
        value["observed"]["list"]["response"]["data"]["items"][0]["name"] = json!("漂移部门");
        fs::write(
            attempt.join("confirmed.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(fixture.history(std::slice::from_ref(&attempt)).is_err());
        fs::write(attempt.join("confirmed.json"), before).unwrap();
        fs::write(attempt.join("unregistered.json"), b"{}").unwrap();
        assert!(fixture.history(std::slice::from_ref(&attempt)).is_err());
    }
}

#[test]
fn confirmed_history_refuses_later_attempts_foreign_files_and_changed_plan() {
    let fixture = Fixture::new();
    let attempt = fixture.attempt(1);
    fixture.confirm(&attempt, true);
    assert!(fixture.history(std::slice::from_ref(&attempt)).is_ok());
    let later = fixture.attempt(2);
    assert!(fixture.history(std::slice::from_ref(&attempt)).is_err());
    assert!(fixture.history(&[attempt.clone(), later]).is_err());
    fs::write(
        fixture.run.join("seed-runtime/department-plan.json"),
        b"{\"fixture\":\"changed\"}",
    )
    .unwrap();
    assert!(
        history::confirmation(
            &fixture.backend,
            &fixture.run,
            &fixture.action,
            &data::target(),
            &attempt
        )
        .is_err()
    );
    let args = json!({"plan":{},"action":fixture.action,"attempt":fixture.run});
    assert!(history::dispatch(&fixture.backend, &fixture.run, &json!("intent"), &args).is_err());
}

#[cfg(windows)]
#[test]
fn history_rejects_junction_attempt_without_changing_preserved_target() {
    use std::process::Command;
    let fixture = Fixture::new();
    let attempt = fixture.attempt(1);
    fixture.confirm(&attempt, false);
    let original = fs::read(attempt.join("confirmed.json")).unwrap();
    let junction = attempt.parent().unwrap().join("0002");
    let output = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&attempt)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture
            .history(&[attempt.clone(), junction.clone()])
            .is_err()
    );
    fs::remove_dir(&junction).unwrap();
    assert_eq!(fs::read(attempt.join("confirmed.json")).unwrap(), original);
}
