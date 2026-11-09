//! 每次 PUT 前保存原始意图，按同次编号保存观察和显式核对。
use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    Port, Prepared, Reads, authorization_shape, history, model, observe, plan_path, present,
};
use crate::{Result, recovery::clone::native::json as document};

impl Prepared<'_> {
    pub(super) fn apply_one(
        &mut self,
        port: &mut impl Port,
        plan: &Value,
        item: &Value,
    ) -> Result<()> {
        let slot = document::text(&item["target"]["slot"], "配额身份槽")?;
        let root = self.run.join("seed-runtime/quotas").join(slot);
        let previous = self.reads.directories(&root)?;
        if let Some(attempt) = previous.last() {
            if present(&attempt.join("confirmed.json"))? {
                return self.verify_after(port, plan, item, attempt);
            }
            if present(&attempt.join("intent.json"))? {
                reconciled_before(&mut self.reads, self.run, item, attempt)?;
            } else {
                partial(&mut self.reads, item, attempt)?;
            }
        }
        let attempt = self.reads.create_attempt(&root, previous.len())?;
        self.check(port)?;
        let before = observe(port, &item["target"])?;
        let before_binding = self.reads.write(&attempt.join("before.json"), &before)?;
        if item["before"] == item["after"] {
            self.check(port)?;
            let observed = self.save_observed(&attempt, &before)?;
            return self.confirm(
                item,
                &attempt,
                &before,
                json!({"observed":observed}),
                false,
                false,
            );
        }
        if model::classification(item, &before)? != "before" {
            return Err("PUT 前完整租户及用户角色用量不同于计划".into());
        }
        self.check(port)?;
        let intent = history::intent_value(&mut self.reads, self.run, item, &attempt)?;
        let intent_binding = self.reads.write(&attempt.join("intent.json"), &intent)?;
        self.reads.check()?;
        let tenant = document::text(&item["target"]["tenant_id"], "配额租户")?;
        let response = port.update(tenant, &intent["body"])?;
        document::exact(&response, &["response", "authorization"], "配额更新响应")?;
        authorization_shape(&response["authorization"])?;
        let response_binding = self
            .reads
            .write(&attempt.join("api-response.json"), &response)?;
        model::authorization(self.post, &response["authorization"])?;
        if *model::data(&response["response"])? != item["after"]["tenant"] {
            return Err("PUT 返回不匹配完整后像；不重放".into());
        }
        let observed = observe(port, &item["target"])?;
        let binding = self.save_observed(&attempt, &observed)?;
        let evidence = json!({"observed":binding,"intent":intent_binding,
            "before":before_binding,"api-response":response_binding});
        self.check(port)?;
        self.confirm(item, &attempt, &observed, evidence, true, true)
    }

    pub(super) fn reconcile_one(
        &mut self,
        port: &mut impl Port,
        plan: &Value,
        item: &Value,
    ) -> Result<()> {
        let paths = super::directories(&mut self.reads, self.run, item)?;
        let Some(attempt) = paths.last() else {
            return Ok(());
        };
        if present(&attempt.join("confirmed.json"))? {
            return self.verify_after(port, plan, item, attempt);
        }
        if !present(&attempt.join("intent.json"))? {
            return Ok(());
        }
        history::intent(&mut self.reads, self.run, item, attempt)?;
        let intent = self.reads.binding(&attempt.join("intent.json"))?;
        let observed = observe(port, &item["target"])?;
        let descriptor = self.save_observed(attempt, &observed)?;
        let state = model::classification(item, &observed)?;
        let path = attempt.join(format!("reconcile-{:04}.json", self.number));
        let reconcile = self.reads.write(
            &path,
            &json!({"intent":intent,"observed":descriptor,"state":state}),
        )?;
        self.check(port)?;
        match state {
            "after" => {
                let before = self.reads.binding(&attempt.join("before.json"))?;
                self.confirm(item, attempt, &observed,
                    json!({"observed":descriptor,"reconcile":reconcile,"intent":intent,"before":before}),
                    true, false)
            }
            "before" => Ok(()),
            _ => Err("未知写入既非完整前像也非完整后像，需要核实现场".into()),
        }
    }

    fn save_observed(&mut self, attempt: &Path, observed: &Value) -> Result<Value> {
        self.reads.write(
            &attempt.join(format!("observed-{:04}.json", self.number)),
            observed,
        )
    }

    fn confirm(
        &mut self,
        item: &Value,
        attempt: &Path,
        observed: &Value,
        evidence: Value,
        attempted: bool,
        proven: bool,
    ) -> Result<()> {
        if model::classification(item, observed)? != "after" {
            return Err("配额完整后像不符，必须核对未知写入".into());
        }
        let plan = self.reads.binding(&plan_path(self.run))?;
        self.reads.write(&attempt.join("confirmed.json"), &json!({"plan":plan,"target":item["target"],
            "observed":observed,"put_attempted":attempted,"api_response_proven":proven,"evidence":evidence}))?;
        Ok(())
    }
}

pub(super) fn reconciled_before(
    reads: &mut Reads<'_>,
    run: &Path,
    item: &Value,
    attempt: &Path,
) -> Result<()> {
    history::intent(reads, run, item, attempt)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(attempt)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("reconcile-"))
        {
            if !history::evidence_name("reconcile", &path) {
                return Err("配额显式核对文件编号无效".into());
            }
            files.push(path);
        }
    }
    files.sort();
    let latest = files
        .last()
        .ok_or("存在未知配额写入，必须先显式核对，不能重放")?;
    let name = latest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("配额核对文件名无效")?;
    let filename = name
        .strip_prefix("reconcile-")
        .ok_or("配额核对文件名无效")?;
    let proof = reads.read(latest)?;
    let observed = attempt.join(format!("observed-{filename}"));
    let expected = json!({"intent":reads.binding(&attempt.join("intent.json"))?,
        "observed":reads.binding(&observed)?,"state":"before"});
    if proof != expected
        || model::classification(item, &reads.bound(&proof["observed"])?)? != "before"
    {
        return Err("上次核对未证明同次意图的原完整前像，禁止新写入".into());
    }
    Ok(())
}

pub(super) fn partial(reads: &mut Reads<'_>, item: &Value, attempt: &Path) -> Result<()> {
    for entry in fs::read_dir(attempt)? {
        let path = entry?.path();
        if !history::evidence_name("before", &path) && !history::evidence_name("observed", &path) {
            return Err("未登记 PUT 的配额目录含未知证据，必须核实".into());
        }
        if model::classification(item, &reads.read(&path)?)? == "mismatch" {
            return Err("未登记 PUT 的部分本地前像已经不符，必须核实".into());
        }
    }
    Ok(())
}
