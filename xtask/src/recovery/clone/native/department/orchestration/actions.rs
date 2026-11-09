//! 已授权部门动作的观察、一次提交和证据编排。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::super::history;
use super::{Port, Prepared, Published, document, model, plan_actions, workflow};
use crate::Result;

impl Prepared<'_> {
    pub(super) fn check_unknown_writes(&self, plan: &Value) -> Result<()> {
        for (action, target) in plan_actions(plan)?.iter().zip(&self.targets) {
            let (_, attempts) = workflow::directories(self.backend, self.run, action)?;
            let Some(last) = attempts.last() else {
                continue;
            };
            if present(&last.join("confirmed.json"))? {
                history::action_history(self.backend, self.run, action, target, &attempts)?;
                continue;
            }
            for attempt in &attempts {
                if present(&attempt.join("confirmed.json"))? {
                    return Err("已确认部门后出现后续尝试，必须先核实".into());
                }
                if present(&attempt.join("intent.json"))? {
                    workflow::reconciled_before(self.backend, self.run, action, target, attempt)?;
                } else if fs::read_dir(attempt)?.any(|entry| {
                    entry.is_err() || entry.is_ok_and(|file| file.file_name() != "before.json")
                }) {
                    return Err("未登记 POST 的部门目录含未知证据，必须先核实".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply(
        &self,
        port: &mut impl Port,
        prior: &Value,
        action: &Value,
        target: &Value,
    ) -> Result<bool> {
        let step = self.workflow("begin-action", json!({"action":action,"target":target}))?;
        if document::boolean(&step["confirmed"], "部门已确认")? {
            let observed = observe(port, action, target)?;
            if model::classify(action, &observed, target)? != "after" {
                return Err("已确认部门发生变化".into());
            }
            return Ok(false);
        }
        self.guard(port, Some(prior), true)?;
        let before = observe(port, action, target)?;
        self.guard(port, Some(prior), true)?;
        self.workflow(
            "write-intent",
            json!({"action":action,"target":target,"attempt":step["attempt"],
            "observed":before,"idempotency_key":step["idempotency_key"]}),
        )?;
        self.check_files()?;
        let response = port.call(
            "create",
            json!({"tenant_id":target["tenant_id"],
            "body":action["body"],"idempotency_key":step["idempotency_key"]}),
        )?;
        self.workflow(
            "write-response",
            json!({"action":action,"target":target,
            "attempt":step["attempt"],"response":response}),
        )?;
        let observed = observe(port, action, target)?;
        let proof = self.workflow(
            "complete-action",
            json!({"action":action,"target":target,
            "attempt":step["attempt"],"observed":observed,"output":self.output}),
        )?;
        self.guard(port, Some(prior), true)?;
        self.workflow(
            "confirm-action",
            json!({"action":action,"target":target,
            "attempt":step["attempt"],"observed":observed,"proof":proof}),
        )?;
        Ok(true)
    }

    pub(super) fn reconcile(
        &self,
        port: &mut impl Port,
        prior: &Value,
        action: &Value,
        target: &Value,
    ) -> Result<Option<&'static str>> {
        let (_, attempts) = workflow::directories(self.backend, self.run, action)?;
        let Some(attempt) = attempts.last() else {
            return Ok(None);
        };
        if present(&attempt.join("confirmed.json"))? {
            history::confirmation(self.backend, self.run, action, target, attempt)?;
            if model::classify(action, &observe(port, action, target)?, target)? != "after" {
                return Err("已确认部门后像漂移".into());
            }
            return Ok(Some("after"));
        }
        if !present(&attempt.join("intent.json"))? {
            return Ok(None);
        }
        history::intent(self.backend, self.run, action, attempt)?;
        let observed = observe(port, action, target)?;
        let proof = self.workflow(
            "reconcile-action",
            json!({"action":action,"target":target,
            "attempt":attempt,"observed":observed,"output":self.output}),
        )?;
        self.guard(port, Some(prior), true)?;
        match proof["state"].as_str() {
            Some("after") => {
                self.workflow(
                    "confirm-action",
                    json!({"action":action,"target":target,
                    "attempt":attempt,"observed":observed,"proof":proof}),
                )?;
                Ok(Some("after"))
            }
            Some("before") => Ok(Some("before")),
            _ => Err("未知部门写入既非完整空前像也非唯一完整后像".into()),
        }
    }

    pub(super) fn verify(&self, port: &mut impl Port, published: &Published) -> Result<Value> {
        let mut confirmed = Vec::new();
        let mut observed = Vec::new();
        for (action, target) in plan_actions(&published.plan)?.iter().zip(&self.targets) {
            let (_, attempts) = workflow::directories(self.backend, self.run, action)?;
            if attempts.is_empty() {
                return Err("部门尚未覆盖全部普通租户".into());
            }
            let expected =
                history::action_history(self.backend, self.run, action, target, &attempts)?;
            self.guard(port, Some(&published.prerequisite), true)?;
            let current = observe(port, action, target)?;
            let proof = self.workflow(
                "verify-action",
                json!({"action":action,"target":target,
                "observed":current,"output":self.output}),
            )?;
            if proof["confirmed"] != expected {
                return Err("部门最终观察期间动作确认变化".into());
            }
            confirmed.push(expected);
            observed.push(proof["observed"].clone());
        }
        Ok(
            json!({"status":"seed_departments_verified","plan":published.descriptor,"confirmed":confirmed,
            "observed":observed,"remote_writes":0,"worker_authorized":false,"restore_qualified":false}),
        )
    }
}

pub(super) fn observe(port: &mut impl Port, action: &Value, target: &Value) -> Result<Value> {
    let tenant = &target["tenant_id"];
    let name = &action["body"]["name"];
    let listed = port.call("list", json!({"tenant_id":tenant,"name":name}))?;
    model::authorization(target, &listed["authorization"], false)?;
    let page = model::page_data(&listed["response"])?;
    let mut desired = document::array(&page["items"], "部门完整目录")?
        .iter()
        .filter(|item| item["name"] == *name);
    let detail = if let Some(item) = desired.next()
        && desired.next().is_none()
    {
        let id = model::snowflake(&item["id"])?;
        port.call("detail", json!({"tenant_id":tenant,"department_id":id}))?
    } else {
        Value::Null
    };
    let template = port.call("template", json!({"tenant_id":tenant}))?;
    let observed = json!({"list":listed,"detail":detail,"template":template});
    model::image(action, &observed, target)?;
    Ok(observed)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
