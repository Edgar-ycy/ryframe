use super::{evidence, receipt, run_lock::RunLock, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "ledger/reconcile.rs"]
mod reconcile;

pub(super) struct Ledger {
    pub(super) root: PathBuf,
    pub(super) state: Value,
    plan: Value,
    persisted: Vec<u8>,
    lock: Option<RunLock>,
    cursor: usize,
}
impl Ledger {
    pub(super) fn open(plan: &Value, root: &Path, mode: &str, token: &str) -> Result<Self> {
        let backend = Path::new(
            plan["environment"]["backend_dir"]
                .as_str()
                .ok_or("身份计划缺少后端路径")?,
        );
        validate_local_test_path(root, backend, LocalTestPathKind::StateDirectory)?;
        fs::create_dir_all(root)?;
        validate_local_test_path(root, backend, LocalTestPathKind::ExistingDirectory)?;
        let lock = RunLock::acquire(root, plan, token)?;
        let opened = (|| {
            let file = root.join("ledger.json");
            let persisted = match evidence::regular(&file)? {
                Some(bytes) => bytes,
                None if mode == "apply" => {
                    let state = json!({"format_version":1,"plan_sha256":plan["plan_sha256"],"status":"applying","entries":[],"roles":{},"users":{}});
                    evidence::new(&file, &state)?;
                    evidence::encode(&state)?
                }
                None => return Err("身份验证缺少已准备账本".into()),
            };
            let state = evidence::parse(&persisted)?;
            Ok((state, persisted))
        })();
        let (state, persisted) = match opened {
            Ok(value) => value,
            Err(error) => {
                lock.release()?;
                return Err(error);
            }
        };
        let mut ledger = Self {
            root: root.into(),
            state,
            plan: plan.clone(),
            persisted,
            lock: Some(lock),
            cursor: 0,
        };
        let checked = ledger.initialize(mode);
        if let Err(error) = checked {
            ledger.close()?;
            return Err(error);
        }
        Ok(ledger)
    }
    fn initialize(&mut self, mode: &str) -> Result<()> {
        if self.state["format_version"] != 1
            || self.state["plan_sha256"] != self.plan["plan_sha256"]
        {
            return Err("身份账本属于其他计划或版本；保持原始证据，禁止写入".into());
        }
        if mode == "apply" {
            if self.inventory()?.0 {
                return Err("已有验证发布 checkpoint，必须通过 verify 收尾".into());
            }
            if self.state["status"] != "applying" && self.state["status"] != "prepared" {
                return Err("身份账本不是可恢复写入状态；禁止重放".into());
            }
            if self.state["status"] == "applying"
                && self.state["entries"]
                    .as_array()
                    .is_some_and(|entries| entries.iter().any(|entry| entry["phase"] == "started"))
            {
                self.unknown()?;
                return Err("身份账本含未知写入，需要人工核对，禁止重放".into());
            }
            reconcile::require_finalized(self)?;
            state::validate(&self.plan, &self.state, false)
        } else {
            state::validate(&self.plan, &self.state, true)
        }
    }
    pub(super) fn current(&self) -> Result<()> {
        if evidence::regular(&self.root.join("ledger.json"))?.as_deref()
            != Some(self.persisted.as_slice())
        {
            return Err("身份账本在写入或发布期间被替换，保留现场".into());
        }
        Ok(())
    }
    pub(super) fn persist(&mut self) -> Result<()> {
        self.persisted =
            evidence::replace(&self.root.join("ledger.json"), &self.state, &self.persisted)?;
        Ok(())
    }
    pub(super) fn take(&mut self, operation: &str, slot: &str) -> Result<Option<Value>> {
        let expected = state::expected(&self.plan)?;
        if expected
            .get(self.cursor)
            .map(|(op, key)| (op.as_str(), key.as_str()))
            != Some((operation, slot))
        {
            return Err("身份执行顺序偏离确定性计划".into());
        }
        let entry = self.state["entries"]
            .as_array()
            .ok_or("身份账本缺少写入列表")?
            .get(self.cursor)
            .cloned();
        self.cursor += 1;
        Ok(entry)
    }
    pub(super) fn started(&mut self, operation: &str, slot: &str) -> Result<()> {
        self.state["entries"]
            .as_array_mut()
            .ok_or("身份账本记录无效")?
            .push(json!({"operation":operation,"slot":slot,"phase":"started"}));
        self.persist()
    }
    pub(super) fn confirmed(&mut self, operation: &str, slot: &str, fields: Value) -> Result<()> {
        let id = state::id(&fields["id"])?;
        if operation == "post_system_roles" {
            self.state["roles"][slot] = json!(id);
        }
        if operation == "post_system_users" {
            self.state["users"][slot] = json!(id);
        }
        let entry = self.state["entries"]
            .as_array_mut()
            .and_then(|entries| entries.last_mut())
            .ok_or("缺少身份写入前意图")?;
        if entry["phase"] != "started" || entry["operation"] != operation || entry["slot"] != slot {
            return Err("身份写入确认与当前意图不同".into());
        }
        let object = entry.as_object_mut().ok_or("身份意图必须为对象")?;
        for (key, value) in fields.as_object().ok_or("身份确认缺少字段")? {
            object.insert(key.clone(), value.clone());
        }
        object.insert("phase".into(), json!("confirmed"));
        self.persist()
    }
    pub(super) fn unknown(&mut self) -> Result<()> {
        self.state["status"] = json!("needs-reconciliation");
        self.persist()
    }
    pub(super) fn prepared(&mut self) -> Result<Value> {
        if self.cursor != 622 {
            return Err("身份执行没有走完全部确定性操作".into());
        }
        self.state["status"] = json!("prepared");
        state::validate(&self.plan, &self.state, true)?;
        self.persist()?;
        Ok(
            json!({"status":"prepared","plan_sha256":self.plan["plan_sha256"],"entries":622,"roles":11,"users":200}),
        )
    }
    pub(super) fn close(&mut self) -> Result<()> {
        if let Some(lock) = self.lock.take() {
            lock.release()?;
        }
        Ok(())
    }
    pub(super) fn names(&self) -> (String, String) {
        super::seed_evidence::names(self.plan["plan_sha256"].as_str().unwrap_or(""), &self.root)
    }
    fn inventory(&self) -> Result<(bool, bool, bool)> {
        let (intent, receipt) = self.names();
        let allowed = [
            intent.clone(),
            format!("{intent}.pending"),
            receipt.clone(),
            format!("{receipt}.pending"),
        ];
        let mut found = (false, false, false);
        for entry in fs::read_dir(&self.root)? {
            let name = entry?
                .file_name()
                .into_string()
                .map_err(|_| "身份证据文件名无效")?;
            let lower = name.to_lowercase();
            if lower.starts_with("identity-publication-") || lower.starts_with("verified-") {
                if !allowed.contains(&name) {
                    return Err("存在计划外身份发布证据，拒绝猜测或覆盖".into());
                }
                found.0 = true;
                found.1 |= name == intent || name == format!("{intent}.pending");
                found.2 |= name == receipt || name == format!("{receipt}.pending");
            }
        }
        Ok(found)
    }
    pub(super) fn begin_verify(&mut self) -> Result<()> {
        if self.inventory()?.0 {
            return Err("已有身份发布 checkpoint，应先恢复收尾".into());
        }
        if self.state["status"] == "prepared" {
            self.state["status"] = json!("verifying");
            self.state["publication_nonce"] = json!(uuid::Uuid::new_v4().to_string());
            self.persist()?;
        }
        if self.state["status"] != "verifying" {
            return Err("身份验证起点不是 verifying 会话".into());
        }
        state::validate(&self.plan, &self.state, true)
    }
    fn projection(&self) -> Value {
        let mut value = self.state.clone();
        let object = value.as_object_mut().expect("账本已校验为对象");
        object.remove("publication_nonce");
        object.remove("verified_receipt");
        object.insert("status".into(), json!("prepared"));
        value
    }
    fn checked_intent(&self, intent: &Value) -> Result<Value> {
        let (_, receipt) = self.names();
        let value = &intent["receipt"];
        let expected = json!({"format_version":1,"kind":"devex-identity-verification-publication","plan_sha256":self.plan["plan_sha256"],"identity_state":self.root,"publication_nonce":self.state["publication_nonce"],"ledger_sha256":evidence::encoded_hash(&self.projection())?,"receipt_file":receipt,"receipt_sha256":evidence::encoded_hash(value)?,"receipt":value});
        if *intent != expected
            || !matches!(
                self.state["status"].as_str(),
                Some("verifying" | "verified")
            )
        {
            return Err("身份发布 checkpoint 与账本、根目录或计划不同".into());
        }
        receipt::validate(value, &self.plan, &self.state, &self.root)?;
        Ok(value.clone())
    }
    pub(super) fn resume(&mut self) -> Result<Option<Value>> {
        self.current()?;
        let found = self.inventory()?;
        if !found.1 {
            if found.2 || self.state["status"] == "verified" {
                return Err("验证发布缺少 checkpoint，禁止远程重验".into());
            }
            return Ok(None);
        }
        let (intent_name, receipt_name) = self.names();
        let path = self.root.join(&intent_name);
        let bytes = evidence::regular(&path)?
            .or(evidence::regular(
                &self.root.join(format!("{intent_name}.pending")),
            )?)
            .ok_or("身份发布 checkpoint 消失")?;
        let intent = evidence::parse(&bytes)?;
        let receipt = self.checked_intent(&intent)?;
        evidence::immutable(&path, &intent)?;
        evidence::immutable(&self.root.join(&receipt_name), &receipt)?;
        self.current()?;
        if self.state["status"] == "verifying" {
            self.state["status"] = json!("verified");
            self.state["verified_receipt"] = json!(receipt_name);
            self.persist()?;
        } else if self.state["verified_receipt"] != receipt_name {
            return Err("verified 账本没有指向确定性收据".into());
        }
        state::validate(&self.plan, &self.state, true)?;
        Ok(Some(receipt))
    }
    pub(super) fn publish(
        &mut self,
        value: &Value,
        checkpoint: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        if self.state["status"] != "verifying" || self.inventory()?.0 {
            return Err("身份发布起点不是唯一 verifying 账本".into());
        }
        state::validate(&self.plan, &self.state, true)?;
        receipt::validate(value, &self.plan, &self.state, &self.root)?;
        let (intent, receipt) = self.names();
        let value = json!({"format_version":1,"kind":"devex-identity-verification-publication","plan_sha256":self.plan["plan_sha256"],"identity_state":self.root,"publication_nonce":self.state["publication_nonce"],"ledger_sha256":evidence::encoded_hash(&self.projection())?,"receipt_file":receipt,"receipt_sha256":evidence::encoded_hash(value)?,"receipt":value});
        evidence::immutable(&self.root.join(intent), &value)?;
        checkpoint("receipt-boundary")?;
        self.current()?;
        evidence::immutable(&self.root.join(&receipt), &value["receipt"])?;
        self.checked_intent(&value)?;
        checkpoint("ledger-boundary")?;
        self.current()?;
        self.state["status"] = json!("verified");
        self.state["verified_receipt"] = json!(receipt);
        self.persist()?;
        checkpoint("outer-result-boundary")?;
        self.current()
    }
}
