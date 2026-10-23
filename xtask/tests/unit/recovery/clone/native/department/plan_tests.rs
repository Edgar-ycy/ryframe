use serde_json::{Value, json};
use std::fs;

use super::{common, digest, document, model_tests as data, plan, projection_tests::Fixture};

fn save(path: &std::path::Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn rehash(value: &mut Value) {
    value.as_object_mut().unwrap().remove("plan_sha256");
    value["plan_sha256"] = json!(digest::plan_hash(value).unwrap());
}

pub(super) fn fixture() -> (Fixture, Value) {
    let mut fixture = Fixture::new();
    fixture.plan["environment"] = json!({"plan_id":"history-plan"});
    for (index, group) in fixture.plan["groups"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        group["kind"] = json!(if index == 0 { "system" } else { "tenant" });
        group["tenant_id"] = json!((100 + index).to_string());
        group["admin"] = json!({"username":"admin"});
        group["permissions"] = json!(["system:user-import:add"]);
        for (member, user) in group["users"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            user["username"] = json!(format!("user-{index}-{member}"));
            user["credential_slot"] = json!("tenant");
            user["client_address"] = json!(format!("198.18.{index}.{}", member + 1));
        }
    }
    rehash(&mut fixture.plan);
    fixture.ledger["plan_sha256"] = fixture.plan["plan_sha256"].clone();
    save(&fixture.run.join("plan.json"), &fixture.plan);
    save(&fixture.root.join("ledger.json"), &fixture.ledger);
    let request = json!({"identity_plan":common::binding(&fixture.run.join("plan.json")).unwrap(),"identity_state":fixture.root});
    (fixture, request)
}

fn invoke(f: &Fixture, request: &Value, method: &str) -> crate::Result<Value> {
    plan::dispatch(
        &f.backend,
        &f.run,
        &json!(method),
        &json!({"request":request}),
    )
}

pub(super) fn department_plan(f: &Fixture, request: &Value) -> Value {
    document::write_new(
        &f.run.join("seed-runtime.json"),
        &json!({"fixture":"registration"}),
    )
    .unwrap();
    fs::create_dir_all(f.run.join("seed-runtime/departments")).unwrap();
    let projection = super::projection(
        &f.backend,
        &f.run,
        &json!({"operation":"identity-projection","request":request,"allow_absent":false}),
    )
    .unwrap();
    document::write_new(&f.run.join("seed-runtime/departments/identity-prerequisite.json"),&json!({"format_version":1,"kind":"devex-seed-identity-prerequisite",
        "registration":common::binding(&f.run.join("seed-runtime.json")).unwrap(),"identity":projection})).unwrap();
    let templates = invoke(f, request, "template-identities").unwrap();
    let output = f.run.join("seed-runtime/attempt-0001");
    fs::create_dir(&output).unwrap();
    let mut actions = Vec::new();
    for (index, target) in plan::targets(&f.plan).unwrap().iter().enumerate() {
        let mut action = data::action();
        action["target"] = json!({"slot":target["slot"],"tenant_id":target["tenant_id"]});
        action["body"] = target["body"].clone();
        action["expected"] = target["expected"].clone();
        action["principal"]["tenant_id"] = target["tenant_id"].clone();
        action["template_principal"] = super::model::principal(&templates[index]).unwrap();
        let mut observed = data::observed(false);
        observed["list"]["authorization"]["tenant_id"] = target["tenant_id"].clone();
        observed["template"]["authorization"] = json!({"subject_id":templates[index]["subject_id"],"tenant_id":target["tenant_id"],
            "username":templates[index]["username"],"is_super_admin":false,"permissions":["system:user-import:add"]});
        let before = output.join(format!("{}-before.json", target["slot"].as_str().unwrap()));
        document::write_new(&before, &observed).unwrap();
        action["before_evidence"] = common::binding(&before).unwrap();
        actions.push(action);
    }
    let mut plan = json!({"format_version":1,"kind":"devex-seed-departments","registration":common::binding(&f.run.join("seed-runtime.json")).unwrap(),
        "identity_plan":request["identity_plan"],"prerequisite":common::binding(&f.run.join("seed-runtime/departments/identity-prerequisite.json")).unwrap(),"actions":actions});
    rehash(&mut plan);
    document::write_new(&f.run.join("seed-runtime/department-plan.json"), &plan).unwrap();
    plan
}

#[test]
fn complete_ten_tenant_plan_uses_verified_622_entry_projection_and_bound_before_images() {
    let (f, request) = fixture();
    let plan = department_plan(&f, &request);
    assert_eq!(invoke(&f, &request, "validate-plan").unwrap(), plan);
    let before = fs::read(f.root.join("ledger.json")).unwrap();
    let identities = invoke(&f, &request, "template-identities").unwrap();
    assert_eq!(identities.as_array().unwrap().len(), 10);
    assert_eq!(identities[0]["subject_id"], "20000");
    assert_eq!(fs::read(f.root.join("ledger.json")).unwrap(), before);
    let path = f.run.join("seed-runtime/department-plan.json");
    for mutation in ["tenant", "subject", "missing", "hash", "before"] {
        let mut changed = plan.clone();
        match mutation {
            "tenant" => changed["actions"][0]["target"]["tenant_id"] = json!("other"),
            "subject" => changed["actions"][0]["template_principal"]["subject_id"] = json!("999"),
            "missing" => {
                changed["actions"].as_array_mut().unwrap().pop();
            }
            "before" => changed["actions"][0]["before"]["total"] = json!(1),
            _ => changed["kind"] = json!("wrong"),
        }
        if mutation != "hash" {
            rehash(&mut changed);
        }
        save(&path, &changed);
        assert!(invoke(&f, &request, "validate-plan").is_err(), "{mutation}");
    }
}

#[test]
fn template_projection_rejects_wildcard_and_ledger_drift_and_prerequisite_locks() {
    let (mut f, mut request) = fixture();
    department_plan(&f, &request);
    fs::create_dir(f.root.join("lock")).unwrap();
    assert!(invoke(&f, &request, "template-identities").is_err());
    fs::remove_dir(f.root.join("lock")).unwrap();
    let original_ledger = f.ledger.clone();
    f.ledger["entries"][0]["id"] = json!("999");
    save(&f.root.join("ledger.json"), &f.ledger);
    assert!(invoke(&f, &request, "template-identities").is_err());
    f.plan["groups"][1]["permissions"] = json!(["*", "system:user-import:add"]);
    rehash(&mut f.plan);
    f.ledger = original_ledger;
    f.ledger["plan_sha256"] = f.plan["plan_sha256"].clone();
    save(&f.root.join("ledger.json"), &f.ledger);
    save(&f.run.join("plan.json"), &f.plan);
    request["identity_plan"] = common::binding(&f.run.join("plan.json")).unwrap();
    assert!(invoke(&f, &request, "template-identities").is_err());
}
