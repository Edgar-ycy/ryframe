//! 失败结果必须等待授权监督器和完整成员关闭，再原样归档。
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    op: Operation,
}
impl Fixture {
    fn new(action: &str) -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::tempdir_in(local).unwrap();
        let directory = operation::canonical(temporary.path()).unwrap();
        let runtime = json!({"scope_id":"worker-reconcile","worker_ready_url":"http://127.0.0.1:19210/readyz","artifacts":{"worker":{"path":std::env::current_exe().unwrap(),"sha256":"d".repeat(64)}}});
        operation::write_new(&directory.join("runtime.json"), &runtime).unwrap();
        let op = Operation::claim(action, &directory, &directory, &runtime, 1.0).unwrap();
        Self {
            _temporary: temporary,
            op,
        }
    }
    fn children(&self, identity: Value, authorized: bool) {
        let mut candidate = self
            .op
            .bound("full-stack-worker-control-candidate")
            .unwrap();
        candidate["identity"] = identity.clone();
        candidate["created_at_ns"] = json!(1);
        self.op
            .publish("candidate.json", &candidate, false)
            .unwrap();
        if authorized {
            let mut supervisor = self
                .op
                .bound("full-stack-worker-control-supervisor")
                .unwrap();
            supervisor["identity"] = identity;
            supervisor["candidate_sha256"] =
                json!(super::super::documents::hash(&candidate).unwrap());
            supervisor["authorized_at_ns"] = json!(2);
            self.op
                .publish("supervisor.json", &supervisor, false)
                .unwrap();
        }
    }
    fn result(&self, success: bool) {
        let value = self
            .op
            .result(
                if success { "running" } else { "unknown" },
                if success {
                    process::current().unwrap()
                } else {
                    Value::Null
                },
                if success {
                    None
                } else {
                    Some("原始就绪失败")
                },
                false,
            )
            .unwrap();
        self.op.publish("result.json", &value, false).unwrap();
    }
    fn proof(&self, supervisor: &Value) {
        let monitor = missing(2147481002);
        let ready = json!({"format_version":1,"operation_id":self.op.owner["operation_id"],"directory":self.op.directory(),"role":"worker","scope_id":self.op.request["scope_id"],"supervisor":supervisor,"monitor":monitor});
        operation::write_new(
            &tree::path(self.op.directory(), self.op.id().unwrap(), "ready"),
            &ready,
        )
        .unwrap();
        let mut proof = ready;
        proof["status"] = json!("stopped");
        proof["members"] = json!([supervisor]);
        proof["error_type"] = Value::Null;
        operation::write_new(
            &tree::path(self.op.directory(), self.op.id().unwrap(), "stopped"),
            &proof,
        )
        .unwrap();
    }
}
fn missing(pid: u32) -> Value {
    json!({"pid":pid,"started":"1","executable":std::env::current_exe().unwrap()})
}

#[test]
fn failed_result_waits_for_live_supervisor_without_releasing_lock() {
    let fixture = Fixture::new("start");
    fixture.children(process::current().unwrap(), true);
    fixture.result(false);
    assert!(archive_completed(&fixture.op).unwrap_err().is::<Pending>());
    assert!(fixture.op.path.exists());
}
#[test]
fn failed_result_archives_only_after_complete_member_cleanup_preserving_original_failure() {
    let fixture = Fixture::new("start");
    let supervisor = missing(2147481001);
    fixture.children(supervisor.clone(), true);
    fixture.result(false);
    assert!(archive_completed(&fixture.op).is_err());
    assert!(fixture.op.path.exists());
    fixture.proof(&supervisor);
    let before = operation::read(&fixture.op.path.join("result.json")).unwrap();
    let after = archive_completed(&fixture.op).unwrap();
    assert_eq!(before, after);
    assert_eq!(after["error"], "原始就绪失败");
    assert!(!fixture.op.path.exists());
}
#[test]
fn cleanup_failure_retains_operation_and_original_error() {
    let fixture = Fixture::new("start");
    let supervisor = missing(2147481001);
    fixture.children(supervisor.clone(), true);
    fixture.result(false);
    fixture.proof(&supervisor);
    let path = tree::path(fixture.op.directory(), fixture.op.id().unwrap(), "stopped");
    let mut proof = operation::read(&path).unwrap();
    proof["status"] = json!("failed");
    json::write_replace(&path, &proof).unwrap();
    assert!(archive_completed(&fixture.op).is_err());
    assert_eq!(
        operation::read(&fixture.op.path.join("result.json")).unwrap()["error"],
        "原始就绪失败"
    );
}
#[test]
fn reused_supervisor_pid_is_rejected_without_archiving() {
    let fixture = Fixture::new("start");
    let mut identity = process::current().unwrap();
    identity["started"] = json!("1");
    fixture.children(identity, true);
    fixture.result(false);
    assert!(
        archive_completed(&fixture.op)
            .unwrap_err()
            .to_string()
            .contains("复用")
    );
    assert!(fixture.op.path.exists());
}
#[test]
fn failed_candidate_waits_without_claiming_authorized_members() {
    let fixture = Fixture::new("start");
    fixture.children(process::current().unwrap(), false);
    fixture.result(false);
    assert!(archive_completed(&fixture.op).unwrap_err().is::<Pending>());
}
#[test]
fn stopped_unauthorized_candidate_needs_no_fabricated_product_proof() {
    let fixture = Fixture::new("start");
    fixture.children(missing(2147481001), false);
    fixture.result(false);
    assert_eq!(archive_completed(&fixture.op).unwrap()["outcome"], "failed");
}
#[test]
fn malformed_result_or_authorization_never_releases_control_lock() {
    for name in ["result.json", "supervisor.json"] {
        let fixture = Fixture::new("start");
        fixture.children(missing(2147481001), true);
        fixture.result(false);
        json::write_replace(&fixture.op.path.join(name), &json!({})).unwrap();
        assert!(archive_completed(&fixture.op).is_err());
        assert!(fixture.op.path.exists());
    }
}
#[test]
fn successful_start_keeps_live_supervisor_and_requires_matching_ready_progress() {
    let fixture = Fixture::new("start");
    fixture.children(process::current().unwrap(), true);
    fixture.result(true);
    assert!(archive_completed(&fixture.op).is_err());
    let mut progress = fixture
        .op
        .bound("full-stack-worker-control-progress")
        .unwrap();
    progress["phase"] = json!("ready");
    progress["identity"] = process::current().unwrap();
    progress["supervisor_sha256"] = json!(
        super::super::documents::hash(&fixture.op.child("supervisor.json").unwrap()).unwrap()
    );
    progress["updated_at_ns"] = json!(3);
    fixture
        .op
        .publish("progress.json", &progress, false)
        .unwrap();
    assert_eq!(archive_completed(&fixture.op).unwrap()["state"], "running");
}
#[test]
fn other_completed_operations_do_not_wait_for_startup() {
    let fixture = Fixture::new("status");
    fixture.result(false);
    assert_eq!(archive_completed(&fixture.op).unwrap()["outcome"], "failed");
}
