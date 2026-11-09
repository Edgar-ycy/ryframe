use std::{collections::BTreeSet, fs};

use serde_json::{Value, json};

use super::{
    common, digest, document, model, model_tests as data,
    orchestration::{Port, Prepared},
    plan, plan_tests,
    projection_tests::Fixture,
    verified_evidence,
};
use crate::{Result, recovery::clone::native::lifecycle::state};

fn fixture() -> (Fixture, Value) {
    let (mut fixture, mut request) = plan_tests::fixture();
    fixture.plan["environment"]["scope_id"] = json!("department-orchestration");
    fixture.plan.as_object_mut().unwrap().remove("plan_sha256");
    fixture.plan["plan_sha256"] = json!(digest::plan_hash(&fixture.plan).unwrap());
    fixture.ledger["plan_sha256"] = fixture.plan["plan_sha256"].clone();
    document::write_replace(&fixture.run.join("plan.json"), &fixture.plan).unwrap();
    document::write_replace(&fixture.root.join("ledger.json"), &fixture.ledger).unwrap();
    request["identity_plan"] = common::binding(&fixture.run.join("plan.json")).unwrap();
    document::write_new(&fixture.run.join("manifest.json"), &json!({})).unwrap();
    state::initialize(&fixture.run).unwrap();
    document::write_new(&fixture.run.join("seed-runtime.json"), &request).unwrap();
    (fixture, request)
}

fn run(fixture: &Fixture, request: &Value, mode: &str, port: &mut Fake) -> Result<Value> {
    let number = state::begin(&fixture.run, "seed-runtime", mode, &json!({}))
        .unwrap()
        .0;
    let output = fixture
        .run
        .join(format!("seed-runtime/attempt-{number:04}"));
    fs::create_dir(&output).unwrap();
    let result = Prepared::load(
        &fixture.backend,
        &fixture.run,
        &output,
        number,
        request,
        mode,
    )
    .and_then(|prepared| prepared.run(port));
    match &result {
        Ok(value) => state::finish(&fixture.run, number, Some(value), None).unwrap(),
        Err(_) => state::finish(&fixture.run, number, None, Some("DepartmentFailure")).unwrap(),
    }
    result
}

struct Fake {
    name: String,
    scope: Value,
    templates: Value,
    tenants: Vec<Value>,
    admin_checks: usize,
    logins: usize,
    drop_first_response: bool,
    posted: Vec<String>,
    departments: BTreeSet<String>,
}

impl Fake {
    fn new(fixture: &Fixture, request: &Value) -> Self {
        let targets = plan::targets(&fixture.plan).unwrap();
        Self {
            name: targets[0]["body"]["name"].as_str().unwrap().into(),
            scope: fixture.plan["environment"]["scope_id"].clone(),
            templates: plan::template_identities(&fixture.backend, &fixture.run, request).unwrap(),
            tenants: targets
                .iter()
                .map(|value| value["tenant_id"].clone())
                .collect(),
            admin_checks: 0,
            logins: 0,
            drop_first_response: true,
            posted: Vec::new(),
            departments: BTreeSet::new(),
        }
    }

    fn observation(&self, tenant: &str) -> Value {
        let template = self
            .templates
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["tenant_id"] == tenant)
            .unwrap();
        let after = self.departments.contains(tenant);
        let mut observation = data::observed(after);
        observation["list"]["authorization"]["tenant_id"] = json!(tenant);
        if after {
            observation["list"]["response"]["data"]["items"][0]["name"] = json!(self.name);
            observation["detail"]["authorization"]["tenant_id"] = json!(tenant);
            observation["detail"]["response"]["data"]["name"] = json!(self.name);
            observation["template"]["response"] = data::template(&[&self.name]);
        }
        observation["template"]["authorization"] = json!({"subject_id":template["subject_id"],
            "tenant_id":tenant,"username":template["username"],"is_super_admin":false,
            "permissions":["system:user-import:add"]});
        observation
    }
}

impl Port for Fake {
    fn check(&self, prerequisite: Option<&Value>, quick: bool) -> Result<()> {
        assert!(!quick || prerequisite.is_some());
        Ok(())
    }

    fn verify_administrator(&mut self) -> Result<()> {
        self.admin_checks += 1;
        Ok(())
    }

    fn login(&mut self, tenants: &Value, templates: &Value) -> Result<Value> {
        assert_eq!(*tenants, json!(self.tenants));
        assert_eq!(*templates, self.templates);
        self.logins += 1;
        let principals = templates
            .as_array()
            .unwrap()
            .iter()
            .map(model::principal)
            .collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"scope_id":self.scope,"tenant_ids":tenants,"department_name":self.name,
            "template_principals":principals}),
        )
    }

    fn call(&mut self, operation: &str, values: Value) -> Result<Value> {
        let tenant = values["tenant_id"].as_str().unwrap();
        assert!(self.tenants.contains(&json!(tenant)));
        match operation {
            "list" => assert_eq!(values["name"], self.name),
            "detail" => assert_eq!(values["department_id"], "5000"),
            "template" => (),
            "create" => {
                assert_eq!(
                    values["body"],
                    json!({"name":self.name,"parent_id":null,"sort":0})
                );
                assert!(uuid::Uuid::parse_str(values["idempotency_key"].as_str().unwrap()).is_ok());
                assert!(self.departments.insert(tenant.into()));
                self.posted.push(tenant.into());
                if std::mem::take(&mut self.drop_first_response) {
                    return Err("模拟部门 POST 响应丢失".into());
                }
                let mut auth = data::auth(true);
                auth["tenant_id"] = json!(tenant);
                let mut row = data::row();
                row["name"] = json!(self.name);
                return Ok(json!({"response":{"code":200,"data":row},"authorization":auth}));
            }
            _ => panic!("未知动作：{operation}"),
        }
        Ok(self.observation(tenant)[operation].clone())
    }
}

#[test]
fn all_four_modes_reuse_action_receipts_and_block_unknown_posts_before_login() {
    let (fixture, request) = fixture();
    let mut port = Fake::new(&fixture, &request);
    let planned = run(&fixture, &request, "departments-plan", &mut port).unwrap();
    assert_eq!(planned["status"], "seed_departments_planned");
    assert_eq!(planned["remote_writes"], 0);
    assert!(port.posted.is_empty());
    assert!(run(&fixture, &request, "departments-apply", &mut port).is_err());
    let attempt = fixture.run.join("seed-runtime/departments/tenant-01/0001");
    let intent = fs::read(attempt.join("intent.json")).unwrap();
    assert!(!attempt.join("confirmed.json").exists());
    let counts = (port.admin_checks, port.logins);
    assert!(run(&fixture, &request, "departments-apply", &mut port).is_err());
    assert_eq!((port.admin_checks, port.logins), counts);
    let reconciled = run(&fixture, &request, "departments-reconcile", &mut port).unwrap();
    assert_eq!(reconciled["remote_writes"], 0);
    assert_eq!(reconciled["confirmed"].as_array().unwrap().len(), 1);
    let confirmed = document::read(&attempt.join("confirmed.json")).unwrap();
    assert_eq!(confirmed["api_response_proven"], false);
    assert!(confirmed["evidence"].get("reconcile").is_some());
    assert_eq!(port.posted, ["101"]);

    let applied = run(&fixture, &request, "departments-apply", &mut port).unwrap();
    assert_eq!(applied["remote_writes"], 9);
    let verified = run(&fixture, &request, "departments-verify", &mut port).unwrap();
    assert_eq!(verified["status"], "seed_departments_verified");
    assert_eq!(verified["remote_writes"], 0);
    assert_eq!(verified["observed"].as_array().unwrap().len(), 10);
    assert_eq!(verified["confirmed"], applied["confirmed"]);
    assert_eq!(port.posted.len(), 10);
    assert_eq!(fs::read(attempt.join("intent.json")).unwrap(), intent);
    let evidence = verified_evidence(&fixture.backend, &fixture.run, &request, None).unwrap();
    assert_eq!(evidence["confirmed"], verified["confirmed"]);

    port.departments.remove("110");
    assert!(run(&fixture, &request, "departments-verify", &mut port).is_err());
    assert_eq!(port.posted.len(), 10);
    assert!(verified_evidence(&fixture.backend, &fixture.run, &request, None).is_err());
}
