//! 缓存端口只采集真实响应并执行原生唯一决策；已有 attempt 永远不再次 EXEC。
use super::{Resources, Transport, document, json, ready};
use crate::Result;
use serde_json::Value;

impl<T: Transport> Resources<'_, T> {
    pub(crate) fn cache_observation(&self, subjects: &Value) -> Result<(Value, Value)> {
        let namespace = format!(
            "ryframe:{{{}}}:",
            document::text(&self.request["target"]["scope_id"], "来源 scope")?
        );
        let plan =
            super::super::model::cache_plan(&json!({"namespace":namespace,"subjects":subjects}))?;
        let mut raw = Vec::new();
        for subject in document::array(&plan, "固定 11 主体缓存计划")? {
            let keys = &subject["keys"];
            let mut types = json!({});
            for name in ["tenant_epoch", "user_version", "snapshot"] {
                types[name] = self.redis(&["TYPE", document::text(&keys[name], "固定缓存键")?])?;
            }
            raw.push(json!({"types":types,
                "tenant_epoch":self.redis(&["GET",document::text(&keys["tenant_epoch"],"tenant epoch 键")?])?,
                "user_version":self.redis(&["GET",document::text(&keys["user_version"],"用户版本键")?])?}));
        }
        let reads = self.policy("cache-read-plans", json!({"plan":plan,"raw":raw}))?;
        for ((subject, row), read) in document::array(&plan, "缓存计划")?
            .iter()
            .zip(&mut raw)
            .zip(document::array(&reads, "缓存读取计划")?)
        {
            let (fields, payload) = if read["snapshot_type"] == "hash" {
                let key = document::text(&subject["keys"]["snapshot"], "快照键")?;
                (
                    self.redis(&["HKEYS", key])?,
                    self.redis(&["HGET", key, document::text(&read["field"], "快照版本字段")?])?,
                )
            } else {
                (json!([]), json!(""))
            };
            row["fields"] = fields;
            row["payload"] = payload;
        }
        let observed = self.policy("cache-observe", json!({"plan":plan,"raw":raw}))?;
        Ok((
            json!({"namespace":namespace,"subjects":subjects,"plan":plan,"before":observed["before"]}),
            observed["comparisons"].clone(),
        ))
    }

    pub(crate) fn cleanup(&self, observation: &Value, comparisons: &Value) -> Result<Value> {
        let directory = self
            .output
            .parent()
            .ok_or("缓存清理缺少 verification 目录")?;
        (self.checkpoint)()?;
        let outcome = ready::policy::cache_transaction::cleanup(&json!({"backend":self.backend,
            "directory":directory,"output":directory.join("cache-cleanup.json"),"request":self.request,
            "environment":{"APP_ENV":"test"},"observation":observation,"comparisons":comparisons}))?;
        (self.checkpoint)()?;
        if outcome["status"] != "confirmed" {
            return Err(document::text(&outcome["message"], "原生缓存清理安全诊断")?
                .to_owned()
                .into());
        }
        self.publish_cleanup(observation)
    }

    pub(crate) fn publish_cleanup(&self, observation: &Value) -> Result<Value> {
        let directory = self
            .output
            .parent()
            .ok_or("缓存清理缺少 verification 目录")?;
        let mut after = Vec::new();
        let keys = self.policy("cache-keys", json!({"plan":observation["plan"]}))?;
        for key in document::array(&keys, "固定 33 键")? {
            after.push(
                json!({"key":key,"type":self.redis(&["TYPE",document::text(key,"固定缓存键")?])?}),
            );
        }
        let value = self.policy(
            "cache-publication",
            json!({"backend":self.backend,"directory":directory,
            "output":directory.join("cache-cleanup.json"),"observation":observation,"after":after,
            "cleaned_at":chrono::Utc::now().to_rfc3339()}),
        )?;
        document::write_new(&directory.join("cache-cleanup.json"), &value)?;
        Ok(value)
    }

    pub(crate) fn resume_cleanup(&self, intent: &Value) -> Result<Value> {
        let directory = self
            .output
            .parent()
            .ok_or("缓存恢复缺少 verification 目录")?;
        let output = directory.join("cache-cleanup.json");
        let frozen = &intent["authorization_cache"];
        let state = ready::policy::cache_attempt::state(
            &json!({"backend":self.backend,"directory":directory,"output":output}),
        )?;
        let keys = self.policy("cache-keys", json!({"plan":frozen["plan"]}))?;
        let mut types = Vec::new();
        for key in document::array(&keys, "固定 33 键")? {
            types.push(self.redis(&["TYPE", document::text(key, "固定缓存键")?])?);
        }
        let (observed, comparisons) =
            if state["status"] == "not_attempted" && types.iter().any(|kind| *kind != "none") {
                self.cache_observation(&frozen["subjects"])?
            } else {
                (Value::Null, Value::Null)
            };
        let decision = ready::policy::cache_attempt::resume(
            &json!({"backend":self.backend,"directory":directory,
            "output":output,"frozen":frozen,"types":types,"observed":observed}),
        )?;
        match decision["action"].as_str() {
            Some("publish") => self.publish_cleanup(frozen),
            Some("execute") => self.cleanup(frozen, &comparisons),
            _ => Err("来源缓存恢复决策无效；保留 attempt/result 和完整前后像，禁止重放".into()),
        }
    }
}
