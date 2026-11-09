use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{evidence, inputs::Reads, model, validate_plan};
use crate::recovery::clone::native::{
    digest, json as document, lifecycle::state, resources::common,
};

#[path = "capacity/orchestration.rs"]
mod orchestration;

#[derive(Clone, Copy)]
enum Mode {
    ReadOnly,
    Proven,
    Reconciled,
}

struct Fixture {
    _directory: TempDir,
    backend: PathBuf,
    run: PathBuf,
    request: Value,
    plan: Value,
    confirmed: Vec<Value>,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("seed-capacity-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let run = directory.path().join("run");
        fs::create_dir_all(run.join("seed-runtime")).unwrap();
        document::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let post =
            json!({"source_admin":{"subject_id":"101","tenant_id":"system","username":"admin"}});
        document::write_new(&run.join("post-copy.json"), &post).unwrap();
        let post = common::binding(&run.join("post-copy.json")).unwrap();
        stage(
            &run,
            "post-copy",
            "register",
            &json!({"status":"post_copy_registered","registration":post}),
        );
        let identity = identity();
        let path = run.join("identity-plan.json");
        document::write_new(&path, &identity).unwrap();
        let request = json!({"identity_plan":common::binding(&path).unwrap(),"post_copy":post});
        document::write_new(&run.join("seed-runtime.json"), &request).unwrap();
        let actions = model::targets(&identity)
            .unwrap()
            .iter()
            .map(|target| {
                let maximum = if matches!(mode, Mode::ReadOnly) {
                    target["max_users"].as_u64().unwrap()
                } else {
                    20
                };
                model::action(
                    target,
                    &observed(target["tenant_id"].as_str().unwrap(), maximum),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let mut plan = json!({"format_version":1,"kind":"devex-seed-quotas",
            "registration":common::binding(&run.join("seed-runtime.json")).unwrap(),
            "identity_plan":request["identity_plan"],"actions":actions});
        plan["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
        let path = run.join("seed-runtime/quota-plan.json");
        document::write_new(&path, &plan).unwrap();
        stage(
            &run,
            "seed-runtime",
            "quotas-plan",
            &json!({"status":"seed_quotas_planned",
            "plan":common::binding(&path).unwrap()}),
        );
        let confirmed = actions
            .iter()
            .map(|item| confirmation(&run, item, mode))
            .collect::<Vec<_>>();
        stage(
            &run,
            "seed-runtime",
            "quotas-apply",
            &json!({"status":"seed_quotas_verified",
            "plan":common::binding(&path).unwrap(),"confirmed":confirmed}),
        );
        Self {
            _directory: directory,
            backend,
            run,
            request,
            plan,
            confirmed,
        }
    }

    fn evidence(&self) -> crate::Result<Value> {
        evidence(&self.backend, &self.run, &self.request, None)
    }

    fn attempt(&self) -> PathBuf {
        self.run.join("seed-runtime/quotas/system/0001")
    }
}

fn stage(run: &Path, category: &str, mode: &str, result: &Value) {
    let number = state::begin(run, category, mode, &json!({})).unwrap().0;
    state::finish(run, number, Some(result), None).unwrap();
}

fn identity() -> Value {
    let mut groups = Vec::new();
    let mut tenants = Vec::new();
    for index in 0..=10 {
        let tenant = if index == 0 {
            "system".to_owned()
        } else {
            format!("capacity-{index:02}")
        };
        let count = if index == 0 { 100 } else { 10 };
        groups.push(
            json!({"slot":if index==0 { "system".to_owned() } else { format!("tenant-{index:02}") },
            "tenant_id":tenant,"kind":if index==0 { "system" } else { "tenant" },
            "count":count,"users":vec![Value::Null;count]}),
        );
        if index != 0 {
            tenants.push(json!({"tenant_id":tenant}));
        }
    }
    json!({"groups":groups,"environment":{"tenants":tenants,
        "quota":{"system_max_users":200,"tenant_max_users":100,"import_headroom_per_tenant":60}}})
}

fn observed(tenant: &str, maximum: u64) -> Value {
    let usage = json!({"tenant_id":tenant,"users":{"used":2,"limit":maximum},"roles":{"used":2,"limit":10}});
    json!({"tenant":{"code":200,"message":"成功","data":{"tenant_id":tenant,
        "status":"enabled","name":"原租户","domain":null,"expire_at":null,"max_users":maximum,
        "max_roles":10,"max_storage_mb":1024,"max_requests_per_min":1000,"usage":usage,
        "expiration_status":"never","capacity_status":"normal"}},
        "usage":{"code":200,"message":"成功","data":usage}})
}

fn confirmation(run: &Path, item: &Value, mode: Mode) -> Value {
    let attempt = run
        .join("seed-runtime/quotas")
        .join(item["target"]["slot"].as_str().unwrap())
        .join("0001");
    fs::create_dir_all(&attempt).unwrap();
    let after = observed(
        item["target"]["tenant_id"].as_str().unwrap(),
        item["target"]["max_users"].as_u64().unwrap(),
    );
    let path = attempt.join("observed-0003.json");
    document::write_new(&path, &after).unwrap();
    let mut files = json!({"observed":common::binding(&path).unwrap()});
    let plan = common::binding(&run.join("seed-runtime/quota-plan.json")).unwrap();
    if !matches!(mode, Mode::ReadOnly) {
        document::write_new(&attempt.join("before.json"), &item["observed"]).unwrap();
        let mut body = item["after"]["tenant"].clone();
        body.as_object_mut().unwrap().remove("tenant_id");
        body.as_object_mut().unwrap().remove("status");
        document::write_new(
            &attempt.join("intent.json"),
            &json!({"plan":plan,"target":item["target"],
            "before":common::binding(&attempt.join("before.json")).unwrap(),"body":body,
            "operation":"put_platform_tenants_by_tenant_id","automatic_retry":false}),
        )
        .unwrap();
        for name in ["before", "intent"] {
            files[name] = common::binding(&attempt.join(format!("{name}.json"))).unwrap();
        }
        if matches!(mode, Mode::Proven) {
            document::write_new(
                &attempt.join("api-response.json"),
                &json!({
                "response":{"code":200,"data":item["after"]["tenant"]},
                "authorization":{"subject_id":"101","tenant_id":"system","username":"admin",
                    "is_super_admin":true,"permissions":[]}}),
            )
            .unwrap();
            files["api-response"] = common::binding(&attempt.join("api-response.json")).unwrap();
        } else {
            document::write_new(
                &attempt.join("reconcile-0003.json"),
                &json!({
                "intent":files["intent"],"observed":files["observed"],"state":"after"}),
            )
            .unwrap();
            files["reconcile"] = common::binding(&attempt.join("reconcile-0003.json")).unwrap();
        }
    }
    let path = attempt.join("confirmed.json");
    document::write_new(&path, &json!({"plan":plan,"target":item["target"],"observed":after,
        "api_response_proven":matches!(mode,Mode::Proven),"put_attempted":!matches!(mode,Mode::ReadOnly),
        "evidence":files})).unwrap();
    common::binding(&path).unwrap()
}

#[test]
fn all_eleven_readonly_confirmations_are_returned_in_order_without_changing_inputs() {
    let fixture = Fixture::new(Mode::ReadOnly);
    let state_before = common::binding(&fixture.run.join("state.json")).unwrap();
    let plan = common::binding(&fixture.run.join("seed-runtime/quota-plan.json")).unwrap();
    let result = fixture.evidence().unwrap();
    assert_eq!(result["confirmed"], json!(fixture.confirmed));
    assert_eq!(result["plan"], plan);
    assert_eq!(
        result["stage"],
        common::binding(&state::result_path(&fixture.run, 3)).unwrap()
    );
    assert_eq!(result["confirmed"].as_array().unwrap().len(), 11);
    assert_eq!(
        common::binding(&fixture.run.join("state.json")).unwrap(),
        state_before
    );
    for descriptor in &fixture.confirmed {
        let value = common::read_bound(&fixture.backend, descriptor, "只读确认").unwrap();
        assert_eq!(value["put_attempted"], false);
        assert_eq!(value["api_response_proven"], false);
    }
}

#[test]
fn newest_incomplete_or_gapped_action_and_failed_outer_never_use_old_success() {
    for kind in ["missing", "newest", "gap", "failed", "failed-plan"] {
        let fixture = Fixture::new(Mode::ReadOnly);
        match kind {
            "missing" => fs::remove_file(fixture.attempt().join("confirmed.json")).unwrap(),
            "newest" => fs::create_dir(fixture.attempt().parent().unwrap().join("0002")).unwrap(),
            "gap" => fs::create_dir(fixture.attempt().parent().unwrap().join("0003")).unwrap(),
            "failed" | "failed-plan" => {
                let mode = if kind == "failed-plan" {
                    "quotas-plan"
                } else {
                    "quotas-reconcile"
                };
                let number = state::begin(&fixture.run, "seed-runtime", mode, &json!({}))
                    .unwrap()
                    .0;
                state::finish(&fixture.run, number, None, Some("Interrupted")).unwrap();
                assert!(
                    evidence(
                        &fixture.backend,
                        &fixture.run,
                        &fixture.request,
                        Some(number)
                    )
                    .is_ok()
                );
            }
            _ => unreachable!(),
        }
        assert!(fixture.evidence().is_err(), "{kind}");
    }
}

#[test]
fn complete_images_and_raw_evidence_bindings_cannot_be_replaced() {
    for raw_change in [false, true] {
        let fixture = Fixture::new(Mode::ReadOnly);
        if raw_change {
            let path = fixture.attempt().join("observed-0003.json");
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b'\n');
            fs::write(&path, &bytes).unwrap();
            assert!(
                fixture
                    .evidence()
                    .unwrap_err()
                    .to_string()
                    .contains("登记摘要")
            );
            assert_eq!(fs::read(path).unwrap(), bytes);
        } else {
            let path = fixture.attempt().join("confirmed.json");
            let mut value = document::read(&path).unwrap();
            value["observed"]["tenant"]["data"]["name"] = json!("其他写入");
            document::write_replace(&path, &value).unwrap();
            assert!(
                fixture
                    .evidence()
                    .unwrap_err()
                    .to_string()
                    .contains("完整后像")
            );
            assert_eq!(document::read(&path).unwrap(), value);
        }
    }
    let fixture = Fixture::new(Mode::ReadOnly);
    let mut plan = fixture.plan.clone();
    plan["actions"][0]["before"]["users"] = json!(999);
    plan.as_object_mut().unwrap().remove("plan_sha256");
    plan["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
    document::write_replace(&fixture.run.join("seed-runtime/quota-plan.json"), &plan).unwrap();
    assert!(
        validate_plan(
            &mut Reads::new(&fixture.backend),
            &fixture.run,
            &fixture.request
        )
        .unwrap_err()
        .to_string()
        .contains("原完整前像")
    );
}

#[test]
fn proven_put_and_unknown_after_reconciliation_require_their_exact_authorization_chain() {
    for mode in [Mode::Proven, Mode::Reconciled] {
        let fixture = Fixture::new(mode);
        assert_eq!(
            fixture.evidence().unwrap()["confirmed"],
            json!(fixture.confirmed)
        );
        let name = if matches!(mode, Mode::Proven) {
            "api-response"
        } else {
            "reconcile"
        };
        let path = fixture.attempt().join(if matches!(mode, Mode::Proven) {
            "api-response.json"
        } else {
            "reconcile-0003.json"
        });
        let mut value = document::read(&path).unwrap();
        if matches!(mode, Mode::Proven) {
            value["authorization"]["subject_id"] = json!("other");
        } else {
            value["state"] = json!("before");
        }
        document::write_replace(&path, &value).unwrap();
        let confirmed = fixture.attempt().join("confirmed.json");
        let mut value = document::read(&confirmed).unwrap();
        value["evidence"][name] = common::binding(&path).unwrap();
        document::write_replace(&confirmed, &value).unwrap();
        assert!(fixture.evidence().is_err());
    }
}

#[test]
fn confirmation_rejects_extra_evidence_and_valid_observations_borrowed_from_another_directory() {
    for extra in [false, true] {
        let fixture = Fixture::new(Mode::ReadOnly);
        let path = fixture.attempt().join("confirmed.json");
        let mut value = document::read(&path).unwrap();
        let expected = if extra {
            value["evidence"]["unexpected"] = value["evidence"]["observed"].clone();
            "证据集合"
        } else {
            let borrowed = fixture.run.join("borrowed.json");
            document::write_new(&borrowed, &value["observed"]).unwrap();
            value["evidence"]["observed"] = common::binding(&borrowed).unwrap();
            "证据越界"
        };
        document::write_replace(&path, &value).unwrap();
        assert!(
            fixture
                .evidence()
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert_eq!(document::read(&path).unwrap(), value);
    }
}

#[test]
fn final_outer_confirmation_order_is_not_a_set() {
    let fixture = Fixture::new(Mode::ReadOnly);
    let mut saved = state::load(&fixture.run, true).unwrap();
    let path = state::result_path(&fixture.run, 3);
    let mut value = document::read(&path).unwrap();
    value["confirmed"].as_array_mut().unwrap().reverse();
    document::write_replace(&path, &value).unwrap();
    saved["attempts"][2]["result"] = common::binding(&path).unwrap();
    document::write_replace(&fixture.run.join("state.json"), &saved).unwrap();
    assert!(
        fixture
            .evidence()
            .unwrap_err()
            .to_string()
            .contains("最新外层阶段")
    );
}
