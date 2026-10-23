use super::execute;
use crate::recovery::clone::native::{digest, json as document, resources::common};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Fixture {
    backend: PathBuf,
    run: PathBuf,
    root: PathBuf,
    plan: Value,
    ledger: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let run = backend
            .join(".local-tests")
            .join(format!("department-projection-{}", uuid::Uuid::new_v4()));
        let root = run.join("seed-runtime/identities");
        fs::create_dir_all(&root).unwrap();
        let mut groups = vec![];
        let mut entries = vec![];
        let mut roles = serde_json::Map::new();
        let mut users = serde_json::Map::new();
        let mut next_user = 20000;
        let mut request_id = 30000;
        for index in 0..11 {
            let slot = if index == 0 {
                "system".to_owned()
            } else {
                format!("tenant-{index:02}")
            };
            let role = (10000 + index).to_string();
            roles.insert(slot.clone(), json!(role));
            for operation in ["post_system_roles", "put_system_roles_by_id_permissions"] {
                entries
                    .push(json!({"operation":operation,"slot":slot,"phase":"confirmed","id":role}));
            }
            let mut members = vec![];
            let count = if index == 0 { 0 } else { 20 };
            for member in 0..count {
                let user_slot = format!("{slot}-u{member:02}");
                let id = next_user.to_string();
                next_user += 1;
                users.insert(user_slot.clone(), json!(id));
                members.push(json!({"user_slot":user_slot}));
                for operation in [
                    "post_system_users",
                    "post_system_users_by_id_password_reset_requests",
                    "post_auth_password_reset_complete",
                ] {
                    let mut entry =
                        json!({"operation":operation,"slot":user_slot,"phase":"confirmed","id":id});
                    if operation == "post_system_users_by_id_password_reset_requests" {
                        entry["request_id"] = json!(request_id.to_string());
                        request_id += 1;
                    }
                    entries.push(entry);
                }
            }
            groups.push(json!({"slot":slot,"users":members}));
        }
        let mut plan = json!({"format_version":1,"kind":"devex-identities","configuration":{},"environment":{},"groups":groups});
        plan["plan_sha256"] = json!(digest::plan_hash(&plan).unwrap());
        let ledger = json!({"format_version":1,"plan_sha256":plan["plan_sha256"],"status":"prepared", "entries":entries,"roles":roles,"users":users});
        document::write_new(&run.join("plan.json"), &plan).unwrap();
        document::write_new(&root.join("ledger.json"), &ledger).unwrap();
        Self {
            backend,
            run,
            root,
            plan,
            ledger,
        }
    }
    fn args(&self, allow_absent: bool) -> Value {
        json!({"operation":"identity-projection","request":{"identity_plan":common::binding(&self.run.join("plan.json")).unwrap(),
            "identity_state":self.root},"allow_absent":allow_absent})
    }
    fn invoke(&self, allow_absent: bool) -> crate::Result<Value> {
        execute(&self.backend, &self.run, &self.args(allow_absent))
    }
    fn ledger(&self, value: &Value) {
        fs::write(
            self.root.join("ledger.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.run).unwrap();
    }
}

#[test]
fn department_projection_uses_complete_ordered_shared_identity_model_and_preserves_bytes() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.root.join("ledger.json")).unwrap();
    let value = fixture.invoke(false).unwrap();
    assert_eq!(value["entries"], 622);
    assert_eq!(value["roles"], 11);
    assert_eq!(value["users"], 200);
    assert_eq!(value["plan_sha256"], fixture.plan["plan_sha256"]);
    assert_eq!(fs::read(fixture.root.join("ledger.json")).unwrap(), before);
    for change in ["order", "id", "request", "unknown", "partial"] {
        let mut changed = fixture.ledger.clone();
        match change {
            "order" => changed["entries"].as_array_mut().unwrap().swap(0, 1),
            "id" => changed["entries"][0]["id"] = json!("0"),
            "request" => {
                changed["entries"][6]["request_id"] = changed["entries"][3]["request_id"].clone()
            }
            "unknown" => changed["unknown"] = json!(true),
            _ => {
                changed["entries"].as_array_mut().unwrap().pop();
            }
        }
        fixture.ledger(&changed);
        assert!(fixture.invoke(false).is_err(), "{change}");
    }
}

#[test]
fn department_projection_rejects_locks_bad_plan_and_unowned_paths_but_proves_absence() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join("lock")).unwrap();
    assert!(fixture.invoke(false).is_err());
    fs::remove_dir(fixture.root.join("lock")).unwrap();
    let mut args = fixture.args(false);
    args["request"]["identity_state"] = json!(fixture.run.join("other"));
    assert!(execute(&fixture.backend, &fixture.run, &args).is_err());
    let mut plan = fixture.plan.clone();
    plan["groups"][0]["slot"] = json!("other");
    fs::write(
        fixture.run.join("plan.json"),
        serde_json::to_vec(&plan).unwrap(),
    )
    .unwrap();
    assert!(fixture.invoke(false).is_err());
    fs::write(
        fixture.run.join("plan.json"),
        serde_json::to_vec(&fixture.plan).unwrap(),
    )
    .unwrap();
    fs::remove_file(fixture.root.join("ledger.json")).unwrap();
    fs::remove_dir(&fixture.root).unwrap();
    assert!(fixture.invoke(false).is_err());
    assert_eq!(fixture.invoke(true).unwrap()["state"], "absent");
    assert!(!fixture.root.exists());
}
