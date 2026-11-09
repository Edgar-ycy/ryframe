//! 完整部门失败计划采用、连续谱系和证据漂移场景。
use super::{
    adoption, common, digest, document, model, plan, plan_tests, projection_tests::Fixture,
};
use serde_json::{Value, json};
use std::fs;

struct Adoption {
    f: Fixture,
    args: Value,
}
impl Adoption {
    fn new() -> Self {
        let (f, request) = plan_tests::fixture();
        plan_tests::department_plan(&f, &request);
        document::write_new(&f.run.join("manifest.json"), &json!({"scope":"department"})).unwrap();
        let source = plan::identity_plan(&f.backend, &request).unwrap();
        let targets = plan::targets(&source).unwrap();
        let templates = plan::template_identities(&f.backend, &f.run, &request).unwrap();
        let authentication = json!({"scope_id":source["environment"]["scope_id"],
            "tenant_ids":targets.iter().map(|t|t["tenant_id"].clone()).collect::<Vec<_>>(),
            "department_name":targets[0]["body"]["name"],"template_principals":templates.as_array().unwrap().iter().map(|t|model::principal(t).unwrap()).collect::<Vec<_>>()});
        let output = f.run.join("seed-runtime/attempt-0001");
        document::write_new(&output.join("authentication.json"), &authentication).unwrap();
        document::write_new(
            &output.join("session-process.json"),
            &json!({"fixture":true}),
        )
        .unwrap();
        let mut state = json!({"attempts":[attempt(1,false),attempt(2,true)]});
        fs::create_dir(f.run.join("run.lock")).unwrap();
        fs::create_dir(f.run.join("seed-runtime/attempt-0002")).unwrap();
        let mut controllers = vec![];
        for node in state["attempts"].as_array_mut().unwrap() {
            controllers.push(controller(&f, node));
        }
        document::write_new(&f.run.join("run.lock/owner.json"), &controllers[1]["owner"]).unwrap();
        failure(&f, &state["attempts"][0]);
        document::write_new(&f.run.join("state.json"), &state).unwrap();
        let args = json!({"request":request,"state":state,"target":null,
            "processes":{"991":null,"992":controllers[1]["owner"]["identity"]},
            "producers":{"1":{"alive":false,"producer_kind":"department-plan","request":request,
                "controller":controllers[0],"receipt":common::binding(&output.join("session-process.json")).unwrap()}}});
        Self { f, args }
    }
    fn call(&self) -> crate::Result<Value> {
        adoption::dispatch(
            &self.f.backend,
            &self.f.run,
            "adoption-evidence",
            &self.args,
        )
    }
    fn next(&mut self, result: Value) {
        let guard=super::execute(&self.f.backend,&self.f.run,&json!({"operation":"publish","output":self.f.run.join("seed-runtime/attempt-0002"),"attempt":2,"target":null})).unwrap();
        let mut result = result;
        result["evidence"]["guard"] = guard;
        let file = self.f.run.join("adopted-result.json");
        document::write_new(&file,&json!({"status":"seed_departments_planned","plan":result["evidence"]["plan"],
            "prerequisite":result["evidence"]["prerequisite"],"adopted":result["evidence"],"remote_writes":0,"worker_authorized":false,"restore_qualified":false})).unwrap();
        let node = &mut self.args["state"]["attempts"][1];
        node["status"] = json!("failed");
        node["finished_at"] = json!("later");
        node["error_type"] = json!("ValueError");
        node["result"] = common::binding(&file).unwrap();
        failure(&self.f, node);
        let next = attempt(3, true);
        let c = controller(&self.f, &next);
        self.args["state"]["attempts"]
            .as_array_mut()
            .unwrap()
            .push(next);
        self.args["processes"]["992"] = Value::Null;
        self.args["processes"]["993"] = c["owner"]["identity"].clone();
        fs::write(
            self.f.run.join("run.lock/owner.json"),
            serde_json::to_vec(&c["owner"]).unwrap(),
        )
        .unwrap();
        fs::create_dir(self.f.run.join("seed-runtime/attempt-0003")).unwrap();
        self.save_state();
    }
    fn save_state(&self) {
        fs::write(
            self.f.run.join("state.json"),
            serde_json::to_vec(&self.args["state"]).unwrap(),
        )
        .unwrap();
    }
}
fn attempt(number: u64, running: bool) -> Value {
    json!({"number":number,"stage":"seed-runtime","mode":"departments-plan","status":if running {"running"} else {"failed"},
        "finished_at":if running {Value::Null} else {json!("then")},"result":null,"error_type":if running {Value::Null} else {json!("TimeoutError")},
        "sources":{"snapshot":{},"worktree_fingerprint":"sha256:fixture","fingerprints":{
            "product":{"sha256":"1".repeat(64),"files":10},"test_tools":{"sha256":format!("{number:x}").repeat(64),"files":20},"support":{"sha256":"3".repeat(64),"files":5}}}})
}
fn controller(f: &Fixture, node: &Value) -> Value {
    let n = node["number"].as_u64().unwrap();
    let mut original = node.clone();
    original["status"] = json!("running");
    original["finished_at"] = Value::Null;
    original["result"] = Value::Null;
    original["error_type"] = Value::Null;
    let value = json!({"format_version":1,"kind":"devex-stage-controller","attempt":n,"attempt_sha256":digest::plan_hash(&original).unwrap(),
        "owner":{"format_version":1,"directory":f.run,"manifest_sha256":common::binding(&f.run.join("manifest.json")).unwrap()["sha256"],
            "identity":{"pid":990+n,"started":(100+n).to_string(),"executable":"python"}}});
    document::write_new(&f.run.join(format!("controller-{n:04}.json")), &value).unwrap();
    value
}
fn failure(f: &Fixture, node: &Value) {
    let n = node["number"].as_u64().unwrap();
    let value = json!({"format_version":1,"kind":"devex-stage-failure","attempt":n,"stage":"seed-runtime","mode":"departments-plan", "error_type":node["error_type"],
        "controller":common::binding(&f.run.join(format!("controller-{n:04}.json"))).unwrap(),
        "frames":[{"file":"tools/python/devex_clone_run.py","function":"execute","line":1}]});
    document::write_new(&f.run.join(format!("failure-{n:04}.json")), &value).unwrap();
}

#[test]
fn complete_failed_plan_is_adopted_with_same_product_and_changed_tools_without_http() {
    let f = Adoption::new();
    let before = fs::read(f.f.root.join("ledger.json")).unwrap();
    let result = f.call().unwrap();
    assert_eq!(result["evidence"]["attempt"], 1);
    assert_eq!(result["evidence"]["before"].as_array().unwrap().len(), 10);
    assert_ne!(
        result["evidence"]["original_fingerprints"]["test_tools"],
        result["evidence"]["current_fingerprints"]["test_tools"]
    );
    assert_eq!(fs::read(f.f.root.join("ledger.json")).unwrap(), before);
    assert!(
        !f.f.run
            .join("seed-runtime/attempt-0002/adoption-guard.json")
            .exists()
    );
}

#[test]
fn outer_failure_retries_original_and_rejects_unknown_intermediate_output() {
    let mut f = Adoption::new();
    let result = f.call().unwrap();
    f.next(result);
    assert_eq!(f.call().unwrap()["evidence"]["attempt"], 1);
    fs::write(
        f.f.run.join("seed-runtime/attempt-0002/unknown.json"),
        b"{}",
    )
    .unwrap();
    assert!(f.call().is_err());
}

#[test]
fn product_process_and_controller_generation_changes_close_adoption() {
    for mutation in 0..5 {
        let mut f = Adoption::new();
        match mutation {
            0 => {
                f.args["state"]["attempts"][1]["sources"]["fingerprints"]["product"]["sha256"] =
                    json!("8".repeat(64))
            }
            1 => f.args["producers"]["1"]["alive"] = json!(true),
            2 => {
                f.args["processes"]["991"] =
                    json!({"pid":991,"started":"101","executable":"python"})
            }
            3 => f.args["processes"]["992"]["started"] = json!("999"),
            _ => {
                f.args["state"]["attempts"][0]["sources"]["fingerprints"]["product"]["files"] =
                    json!(true)
            }
        }
        f.save_state();
        assert!(f.call().is_err(), "{mutation}");
    }
}

#[test]
fn missing_changed_cross_attempt_or_action_evidence_cannot_be_adopted() {
    for mutation in 0..4 {
        let f = Adoption::new();
        let file = f.f.run.join("seed-runtime/department-plan.json");
        let mut plan = document::read(&file).unwrap();
        let before = std::path::PathBuf::from(
            plan["actions"][0]["before_evidence"]["path"]
                .as_str()
                .unwrap(),
        );
        match mutation {
            0 => fs::remove_file(&before).unwrap(),
            1 => fs::write(&before, b"{}").unwrap(),
            2 => {
                let crossed =
                    f.f.run
                        .join("seed-runtime/attempt-0002/tenant-01-before.json");
                fs::copy(&before, &crossed).unwrap();
                plan["actions"][0]["before_evidence"] = common::binding(&crossed).unwrap();
                plan.as_object_mut().unwrap().remove("plan_sha256");
                plan["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
                fs::write(file, serde_json::to_vec(&plan).unwrap()).unwrap();
            }
            _ => fs::create_dir(f.f.run.join("seed-runtime/departments/tenant-01")).unwrap(),
        }
        assert!(f.call().is_err(), "{mutation}");
    }
}

#[test]
fn stale_history_and_changed_authentication_fail_without_guard_publication() {
    let mut f = Adoption::new();
    f.args["state"]["attempts"][1]["number"] = json!(3);
    assert!(f.call().is_err());
    let f = Adoption::new();
    fs::write(
        f.f.run
            .join("seed-runtime/attempt-0001/authentication.json"),
        b"{}",
    )
    .unwrap();
    assert!(f.call().is_err());
    assert!(
        !f.f.run
            .join("seed-runtime/attempt-0002/adoption-guard.json")
            .exists()
    );
}

fn empty_retry(f: &mut Adoption, quiet: bool) {
    let node = &mut f.args["state"]["attempts"][1];
    node["status"] = json!("failed");
    node["finished_at"] = json!("later");
    node["error_type"] = json!("ValueError");
    failure(&f.f, node);
    let file = f.f.run.join("failure-0002.json");
    let mut failure = document::read(&file).unwrap();
    let mut frames = vec![
        ("tools/python/devex_clone_run.py", "execute"),
        ("tools/python/devex_clone_seed_runtime.py", "execute_seed"),
    ];
    if quiet {
        frames.push(("tools/python/devex_clone_post_process.py", "require_quiet"));
        fs::remove_dir(f.f.run.join("seed-runtime/attempt-0002")).unwrap();
    } else {
        frames.extend([
            (
                "tools/python/devex_clone_department.py",
                "execute_departments",
            ),
            (
                "tools/python/devex_clone_department.py",
                "adopt_failed_plan",
            ),
            (
                "tools/python/devex_clone_department.py",
                "_adoption_evidence",
            ),
            ("tools/python/devex_clone_department.py", "native"),
        ]);
        document::write_new(&f.f.run.join(format!("seed-runtime/attempt-0002/command-{}.json", "a".repeat(32))),
            &json!({"command":["cargo","-V"],"returncode":0,"error_type":null,"stdout":"cargo fixture","stderr":""})).unwrap();
    }
    failure["frames"] =
        json!(frames.iter().enumerate().map(|(index,(file,function))|
        json!({"file":file,"function":function,"line":index+1})).collect::<Vec<_>>());
    fs::write(file, serde_json::to_vec(&failure).unwrap()).unwrap();
    let next = attempt(3, true);
    let c = controller(&f.f, &next);
    f.args["state"]["attempts"]
        .as_array_mut()
        .unwrap()
        .push(next);
    f.args["processes"]["992"] = Value::Null;
    f.args["processes"]["993"] = c["owner"]["identity"].clone();
    fs::write(
        f.f.run.join("run.lock/owner.json"),
        serde_json::to_vec(&c["owner"]).unwrap(),
    )
    .unwrap();
    fs::create_dir(f.f.run.join("seed-runtime/attempt-0003")).unwrap();
    f.save_state();
}

#[test]
fn quiet_and_read_only_guard_failures_keep_the_original_plan_without_remote_reads() {
    for quiet in [true, false] {
        let mut f = Adoption::new();
        empty_retry(&mut f, quiet);
        assert_eq!(f.call().unwrap()["evidence"]["attempt"], 1);
    }
}

#[test]
fn guard_only_failure_cannot_hide_a_producer_or_unknown_write_evidence() {
    for name in ["session-process.json", "unknown.json"] {
        let mut f = Adoption::new();
        empty_retry(&mut f, false);
        document::write_new(
            &f.f.run.join("seed-runtime/attempt-0002").join(name),
            &json!({"fixture":true}),
        )
        .unwrap();
        assert!(f.call().is_err());
    }
}

#[test]
fn original_plan_logout_diagnostic_matches_its_failure_protocol_version() {
    for (version, expected, wrong) in [
        (1, "ValueError", "ProducerLogoutError"),
        (2, "ProducerLogoutError", "ValueError"),
    ] {
        let f = Adoption::new();
        let failure = f.f.run.join("failure-0001.json");
        if version == 2 {
            let mut native = document::read(&failure).unwrap();
            native["format_version"] = json!(2);
            native.as_object_mut().unwrap().remove("frames");
            native["operation_id"] = json!("clone.seed-runtime.departments-plan");
            fs::write(&failure, serde_json::to_vec(&native).unwrap()).unwrap();
        }
        let logout =
            f.f.run
                .join("seed-runtime/attempt-0001/department-logout-error.json");
        document::write_new(&logout, &json!({"error_type":expected})).unwrap();
        assert!(f.call().is_ok());
        fs::write(
            logout,
            serde_json::to_vec(&json!({"error_type":wrong})).unwrap(),
        )
        .unwrap();
        assert!(f.call().is_err());
    }
}
