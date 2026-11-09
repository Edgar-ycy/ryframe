use std::collections::BTreeMap;

use super::{
    PathBuf, TempDir, Value, common, document, fs, identity, json, model, observed, stage, state,
};
use crate::{
    Result,
    recovery::clone::native::seed::capacity::orchestration::{Port, Prepared},
};

struct Run {
    _dir: TempDir,
    backend: PathBuf,
    run: PathBuf,
    post: Value,
    request: Value,
}

impl Run {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("seed-quota-orchestration-")
            .tempdir_in(backend.join(".local-tests"))
            .unwrap();
        let run = directory.path().join("run");
        fs::create_dir_all(run.join("seed-runtime")).unwrap();
        document::write_new(&run.join("manifest.json"), &json!({})).unwrap();
        state::initialize(&run).unwrap();
        let post =
            json!({"source_admin":{"subject_id":"101","tenant_id":"system","username":"admin"}});
        document::write_new(&run.join("post-copy.json"), &post).unwrap();
        let registration = common::binding(&run.join("post-copy.json")).unwrap();
        stage(
            &run,
            "post-copy",
            "register",
            &json!({"status":"post_copy_registered","registration":registration}),
        );
        let path = run.join("identity-plan.json");
        document::write_new(&path, &identity()).unwrap();
        let request = json!({"identity_plan":common::binding(&path).unwrap(),"post_copy":registration,
            "identity_state":run.join("seed-runtime/identities")});
        document::write_new(&run.join("seed-runtime.json"), &request).unwrap();
        Self {
            _dir: directory,
            backend,
            run,
            post,
            request,
        }
    }

    fn execute(&self, mode: &str, port: &mut Fake) -> Result<Value> {
        let number = state::begin(&self.run, "seed-runtime", mode, &json!({}))
            .unwrap()
            .0;
        let output = self
            .run
            .join("seed-runtime")
            .join(format!("attempt-{number:04}"));
        fs::create_dir(&output).unwrap();
        let result = Prepared::load(
            &self.backend,
            &self.run,
            &output,
            number,
            &self.post,
            &self.request,
            mode,
        )
        .and_then(|prepared| prepared.run(port));
        match &result {
            Ok(value) => state::finish(&self.run, number, Some(value), None).unwrap(),
            Err(_) => state::finish(&self.run, number, None, Some("QuotaFailure")).unwrap(),
        }
        result
    }

    fn attempt(&self, number: usize) -> PathBuf {
        self.run
            .join(format!("seed-runtime/quotas/system/{number:04}"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    Normal,
    MissingBefore,
    MissingAfter,
}

struct Fake {
    quotas: BTreeMap<String, u64>,
    reply: Reply,
    admin_checks: usize,
    logins: usize,
    updates: Vec<String>,
}

impl Fake {
    fn new() -> Self {
        let quotas = model::targets(&identity())
            .unwrap()
            .iter()
            .map(|item| {
                let tenant = item["tenant_id"].as_str().unwrap();
                (tenant.to_owned(), if tenant == "system" { 20 } else { 100 })
            })
            .collect();
        Self {
            quotas,
            reply: Reply::Normal,
            admin_checks: 0,
            logins: 0,
            updates: Vec::new(),
        }
    }
}

fn auth() -> Value {
    json!({"subject_id":"101","tenant_id":"system","username":"admin","is_super_admin":true,"permissions":[]})
}

impl Port for Fake {
    fn check(&self) -> Result<()> {
        Ok(())
    }
    fn verify_administrator(&mut self) -> Result<()> {
        self.admin_checks += 1;
        Ok(())
    }
    fn login(&mut self, tenant_ids: &Value) -> Result<Value> {
        assert_eq!(
            tenant_ids,
            &json!(
                std::iter::once("system".to_owned())
                    .chain((1..=10).map(|i| format!("capacity-{i:02}")))
                    .collect::<Vec<_>>()
            )
        );
        self.logins += 1;
        Ok(auth())
    }
    fn get(&mut self, tenant: &str) -> Result<Value> {
        Ok(observed(tenant, self.quotas[tenant])["tenant"].clone())
    }
    fn usage(&mut self, tenant: &str) -> Result<Value> {
        Ok(observed(tenant, self.quotas[tenant])["usage"].clone())
    }
    fn update(&mut self, tenant: &str, body: &Value) -> Result<Value> {
        assert_eq!(tenant, "system");
        assert_eq!(body["max_users"], 200);
        assert_eq!(body.as_object().unwrap().len(), 7);
        self.updates.push(tenant.into());
        if self.reply != Reply::MissingBefore {
            self.quotas.insert(tenant.into(), 200);
        }
        if self.reply != Reply::Normal {
            return Err("模拟未知 PUT".into());
        }
        let mut data = self.get(tenant)?["data"].clone();
        for key in ["usage", "expiration_status", "capacity_status"] {
            data.as_object_mut().unwrap().remove(key);
        }
        Ok(json!({"response":{"code":200,"data":data},"authorization":auth()}))
    }
}

#[test]
fn unknown_put_requires_reconcile_before_any_apply_login_and_after_is_never_replayed() {
    let run = Run::new();
    let mut port = Fake::new();
    run.execute("quotas-plan", &mut port).unwrap();
    port.reply = Reply::MissingAfter;
    assert!(run.execute("quotas-apply", &mut port).is_err());
    let intent = fs::read(run.attempt(1).join("intent.json")).unwrap();
    assert!(!run.attempt(1).join("confirmed.json").exists());
    let calls = (port.admin_checks, port.logins);
    assert!(run.execute("quotas-apply", &mut port).is_err());
    assert_eq!((port.admin_checks, port.logins), calls);

    let reconciled = run.execute("quotas-reconcile", &mut port).unwrap();
    assert_eq!(reconciled["remote_writes"], 0);
    let confirmation = document::read(&run.attempt(1).join("confirmed.json")).unwrap();
    assert_eq!(confirmation["put_attempted"], true);
    assert_eq!(confirmation["api_response_proven"], false);
    assert!(confirmation["evidence"].get("reconcile").is_some());
    let applied = run.execute("quotas-apply", &mut port).unwrap();
    assert_eq!(applied["status"], "seed_quotas_verified");
    assert_eq!(applied["confirmed"].as_array().unwrap().len(), 11);
    let proof = super::evidence(&run.backend, &run.run, &run.request, None).unwrap();
    assert_eq!(proof["confirmed"], applied["confirmed"]);
    assert_eq!(port.updates, ["system"]);
    assert_eq!(
        fs::read(run.attempt(1).join("intent.json")).unwrap(),
        intent
    );
    assert!(!run.attempt(2).exists());
}

#[test]
fn reconciled_before_allows_only_new_attempt_and_plan_replay_keeps_exact_raw_plan() {
    let run = Run::new();
    let mut port = Fake::new();
    let first = run.execute("quotas-plan", &mut port).unwrap();
    let path = run.run.join("seed-runtime/quota-plan.json");
    let raw = fs::read(&path).unwrap();
    assert_eq!(run.execute("quotas-plan", &mut port).unwrap(), first);
    assert_eq!(fs::read(&path).unwrap(), raw);
    assert!(port.updates.is_empty());
    port.reply = Reply::MissingBefore;
    assert!(run.execute("quotas-apply", &mut port).is_err());
    let old_intent = fs::read(run.attempt(1).join("intent.json")).unwrap();
    let result = run.execute("quotas-reconcile", &mut port).unwrap();
    assert_eq!(result["status"], "seed_quotas_reconciled");
    assert_eq!(result["confirmed"], json!([]));
    assert!(!run.attempt(1).join("confirmed.json").exists());
    port.reply = Reply::Normal;
    let result = run.execute("quotas-apply", &mut port).unwrap();
    assert_eq!(result["status"], "seed_quotas_verified");
    assert_eq!(port.updates, ["system", "system"]);
    let proof = super::evidence(&run.backend, &run.run, &run.request, None).unwrap();
    assert_eq!(proof["confirmed"], result["confirmed"]);
    assert_eq!(
        fs::read(run.attempt(1).join("intent.json")).unwrap(),
        old_intent
    );
    let confirm = document::read(&run.attempt(2).join("confirmed.json")).unwrap();
    assert_eq!(confirm["api_response_proven"], true);
}

#[test]
fn mismatched_observation_is_saved_and_cannot_unlock_apply() {
    let run = Run::new();
    let mut port = Fake::new();
    run.execute("quotas-plan", &mut port).unwrap();
    port.reply = Reply::MissingBefore;
    assert!(run.execute("quotas-apply", &mut port).is_err());
    port.quotas.insert("system".into(), 30);
    assert!(run.execute("quotas-reconcile", &mut port).is_err());
    let receipt = document::read(&run.attempt(1).join("reconcile-0004.json")).unwrap();
    assert_eq!(receipt["state"], "mismatch");
    assert!(run.attempt(1).join("observed-0004.json").exists());
    let logins = port.logins;
    assert!(run.execute("quotas-apply", &mut port).is_err());
    assert_eq!(port.logins, logins);
    assert_eq!(port.updates, ["system"]);
}
