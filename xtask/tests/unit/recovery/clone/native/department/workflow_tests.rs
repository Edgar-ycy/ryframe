//! 部门写入未知结果、幂等恢复、主体与完整后像的等价场景。
use super::{document, model_tests as data, workflow};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
    output: PathBuf,
    action: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend.join(".local-tests").join(format!(
            "department-workflow-中文 空格-{}",
            uuid::Uuid::new_v4()
        ));
        let output = run.join("seed-runtime/attempt-0001");
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(run.join("seed-runtime/departments")).unwrap();
        document::write_new(
            &run.join("seed-runtime/department-plan.json"),
            &json!({"fixture":"plan"}),
        )
        .unwrap();
        Self {
            backend,
            run,
            output,
            action: data::action(),
        }
    }
    fn call(&self, method: &str, args: Value) -> crate::Result<Value> {
        workflow::dispatch(&self.backend, &self.run, method, &args)
    }
    fn begin(&self) -> Value {
        self.call(
            "begin-action",
            json!({"action":self.action,"target":data::target()}),
        )
        .unwrap()
    }
    fn intent(&self, step: &Value, observed: Value) -> crate::Result<Value> {
        self.call("write-intent",json!({"action":self.action,"target":data::target(),"attempt":step["attempt"],"observed":observed,"idempotency_key":step["idempotency_key"]}))
    }
    fn response(&self, step: &Value) -> crate::Result<Value> {
        self.call("write-response",json!({"action":self.action,"target":data::target(),"attempt":step["attempt"],"response":{"response":{"code":200,"data":data::row()},"authorization":data::auth(true)}}))
    }
    fn observed(&self, method: &str, step: &Value, observed: Value) -> crate::Result<Value> {
        self.call(method,json!({"action":self.action,"target":data::target(),"attempt":step["attempt"],"observed":observed,"output":self.output}))
    }
    fn confirm(&self, step: &Value, observed: Value, proof: Value) -> crate::Result<Value> {
        self.call("confirm-action",json!({"action":self.action,"target":data::target(),"attempt":step["attempt"],"observed":observed,"proof":proof}))
    }
    fn advance(&mut self, number: u32) {
        self.output = self.run.join(format!("seed-runtime/attempt-{number:04}"));
        fs::create_dir(&self.output).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn unknown_before_requires_explicit_complete_reconciliation_and_reuses_only_original_key() {
    let mut f = Fixture::new();
    let first = f.begin();
    f.intent(&first, data::observed(false)).unwrap();
    assert!(
        f.call(
            "begin-action",
            json!({"action":f.action,"target":data::target()})
        )
        .is_err()
    );
    f.advance(2);
    let proof = f
        .observed("reconcile-action", &first, data::observed(false))
        .unwrap();
    assert_eq!(proof["state"], "before");
    f.advance(3);
    let second = f.begin();
    assert_eq!(second["idempotency_key"], first["idempotency_key"]);
    assert_ne!(second["attempt"], first["attempt"]);
    f.intent(&second, data::observed(false)).unwrap();
    f.response(&second).unwrap();
    let proof = f
        .observed("complete-action", &second, data::observed(true))
        .unwrap();
    let confirmation = f.confirm(&second, data::observed(true), proof).unwrap();
    assert_eq!(confirmation["api_response_proven"], true);
    let already = f.begin();
    assert_eq!(already["confirmed"], true);
    assert_eq!(already["attempt"], second["attempt"]);
}

#[test]
fn unknown_after_is_confirmed_only_by_reconcile_and_does_not_allocate_another_write() {
    let mut f = Fixture::new();
    let step = f.begin();
    f.intent(&step, data::observed(false)).unwrap();
    f.advance(2);
    let proof = f
        .observed("reconcile-action", &step, data::observed(true))
        .unwrap();
    assert_eq!(proof["state"], "after");
    assert_eq!(proof["response_proven"], false);
    let confirmed = f.confirm(&step, data::observed(true), proof).unwrap();
    assert!(confirmed["evidence"].get("api-response").is_none());
    assert!(confirmed["evidence"].get("reconcile").is_some());
    assert_eq!(f.begin()["confirmed"], true);
}

#[test]
fn saved_success_response_must_match_whole_after_image_and_is_never_orphaned() {
    let mut f = Fixture::new();
    let step = f.begin();
    f.intent(&step, data::observed(false)).unwrap();
    f.response(&step).unwrap();
    assert!(
        f.call(
            "begin-action",
            json!({"action":f.action,"target":data::target()})
        )
        .is_err()
    );
    f.advance(2);
    let proof = f
        .observed("reconcile-action", &step, data::observed(true))
        .unwrap();
    assert_eq!(proof["response_proven"], true);
    assert!(proof["reconcile"].is_null());
    f.confirm(&step, data::observed(true), proof).unwrap();
    let path = PathBuf::from(step["attempt"].as_str().unwrap());
    assert!(!path.join("reconcile-0002.json").exists());
    assert!(path.join("api-response.json").is_file());
}

#[test]
fn subject_tenant_template_and_page_changes_prevent_intent_and_keep_partial_attempt() {
    for mutation in 0..4 {
        let f = Fixture::new();
        let step = f.begin();
        let mut before = data::observed(false);
        match mutation {
            0 => before["list"]["authorization"]["subject_id"] = json!("999"),
            1 => before["list"]["authorization"]["tenant_id"] = json!("102"),
            2 => before["template"]["authorization"]["subject_id"] = json!("999"),
            _ => before["list"]["response"]["data"]["total_pages"] = json!(1),
        }
        assert!(f.intent(&step, before).is_err());
        let attempt = PathBuf::from(step["attempt"].as_str().unwrap());
        assert!(attempt.join("before.json").is_file());
        assert!(!attempt.join("intent.json").exists());
    }
}

#[test]
fn unrelated_department_tree_is_preserved_in_full_before_and_after_images() {
    let mut f = Fixture::new();
    let parent = json!({"id":"6998","name":"既有根","parent_id":null,"ancestors":"0","sort":0,"status":"1","remark":null,"created_at":"2026-10-23"});
    let child = json!({"id":"6999","name":"既有子项","parent_id":"6998","ancestors":"6998","sort":1,"status":"1","remark":null,"created_at":"2026-10-23"});
    f.action["before"] = json!({"total":2,"departments":[parent,child]});
    f.action["before_template_paths"] = json!(["既有根"]);
    let mut before = data::observed(false);
    before["list"]["response"]["data"] = json!({"items":[parent,child],"page":1,"page_size":100,"max_page_size":100,"total":2,"total_pages":1});
    before["template"]["response"] = data::template(&["既有根"]);
    let step = f.begin();
    f.intent(&step, before).unwrap();
    f.response(&step).unwrap();
    let mut after = data::observed(true);
    after["list"]["response"]["data"]["items"] = json!([data::row(), parent, child]);
    after["list"]["response"]["data"]["total"] = json!(3);
    after["template"]["response"] = data::template(&["固定部门", "既有根"]);
    let proof = f.observed("complete-action", &step, after.clone()).unwrap();
    f.confirm(&step, after, proof).unwrap();
}

#[test]
fn mismatched_success_response_or_changed_template_never_publishes_confirmation() {
    for mutation in 0..3 {
        let mut f = Fixture::new();
        let step = f.begin();
        f.intent(&step, data::observed(false)).unwrap();
        f.response(&step).unwrap();
        let mut after = data::observed(true);
        match mutation {
            0 => after["list"]["response"]["data"]["items"][0]["id"] = json!("5001"),
            1 => after["template"]["response"] = data::template(&["固定部门", "意外路径"]),
            _ => after["detail"]["authorization"]["subject_id"] = json!("999"),
        }
        assert!(f.observed("complete-action", &step, after).is_err());
        f.advance(2);
        assert!(
            !PathBuf::from(step["attempt"].as_str().unwrap())
                .join("confirmed.json")
                .exists()
        );
    }
}

#[test]
fn unknown_files_noncontiguous_paths_and_cross_slot_attempts_fail_closed() {
    let f = Fixture::new();
    let step = f.begin();
    let attempt = PathBuf::from(step["attempt"].as_str().unwrap());
    document::write_new(&attempt.join("unknown.json"), &json!({"remote_writes":0})).unwrap();
    assert!(
        f.call(
            "begin-action",
            json!({"action":f.action,"target":data::target()})
        )
        .is_err()
    );
    let mut cross = step;
    cross["attempt"] = json!(
        attempt
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tenant-02/0001")
    );
    assert!(f.intent(&cross, data::observed(false)).is_err());
    fs::create_dir(attempt.parent().unwrap().join("0003")).unwrap();
    assert!(workflow::directories(&f.backend, &f.run, &f.action).is_err());
}

#[test]
fn confirmation_checks_bound_observation_and_plan_drift() {
    let f = Fixture::new();
    let step = f.begin();
    f.intent(&step, data::observed(false)).unwrap();
    f.response(&step).unwrap();
    let proof = f
        .observed("complete-action", &step, data::observed(true))
        .unwrap();
    let path = PathBuf::from(proof["observed"]["path"].as_str().unwrap());
    fs::write(&path, b"{}").unwrap();
    assert!(f.confirm(&step, data::observed(true), proof).is_err());
    assert!(
        !PathBuf::from(step["attempt"].as_str().unwrap())
            .join("confirmed.json")
            .exists()
    );
}

#[test]
fn fixed_department_name_conflicts_and_duplicate_rows_cannot_be_planned_or_written() {
    for mutation in 0..3 {
        let f = Fixture::new();
        let mut observed = data::observed(true);
        match mutation {
            0 => observed["list"]["response"]["data"]["items"][0]["status"] = json!("0"),
            1 => {
                observed["list"]["response"]["data"]["items"] = json!([data::row(), data::row()]);
                observed["list"]["response"]["data"]["total"] = json!(2);
            }
            _ => {}
        }
        assert!(f.call("plan-action",json!({"action":f.action,"target":data::target(),"observed":observed,"output":f.output})).is_err());
        assert!(!f.run.join("seed-runtime/departments/tenant-01").exists());
    }
}
