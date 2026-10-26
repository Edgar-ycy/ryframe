//! 同代原生生产者登记、ready 与完成证据；不根据进程退出猜测业务成功。
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use serde_json::{Value, json};

use super::{documents, inventory, ready, staging};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, resources::common, source},
};

#[path = "producer/launch.rs"]
pub(super) mod launch;
#[path = "producer/process.rs"]
mod process;

pub(super) struct Inputs<'a> {
    pub backend: &'a Path,
    pub execution: &'a Path,
    pub directory: &'a Path,
    pub start: &'a Value,
    pub lineage: &'a Value,
    pub environment: &'a BTreeMap<String, String>,
    pub tool: &'a Path,
    pub coordinator: &'a Value,
    pub execution_sha: &'a str,
    pub historical: bool,
}

impl Inputs<'_> {
    pub(super) fn run(&self, checkpoint: &mut dyn FnMut() -> Result<()>) -> Result<Value> {
        process::run(self, checkpoint)
    }
    fn policy(&self, action: &str, arguments: Value) -> Result<Value> {
        ready::verify(&json!({"action":action,"arguments":arguments}))
    }

    fn source_unchanged(&self) -> Result<()> {
        // 历史来源的严格形状和干净状态由同一 staging 模型核验；不能与当前 checkout 混同。
        if !self.historical && source::current(self.backend)? != *self.coordinator {
            return Err("协调器必须保持 START 登记的干净 SHA 和完整工具来源".into());
        }
        Ok(())
    }

    fn staging(&self, intent: &Value) -> Result<Value> {
        let actual = common::binding(self.tool)?;
        staging::verify(
            &json!({"coordinator":self.backend,"execution":self.execution,
            "directory":self.directory,"descriptor":intent["staging"],"coordinator_source":self.coordinator,
            "execution_sha":self.execution_sha,"tool":{"path":self.tool,"sha256":actual["sha256"]}}),
        )
    }

    pub(super) fn registered(&self) -> Result<Value> {
        let intent = self.read("source-producer-intent.json")?;
        self.source_unchanged()?;
        let staged = self.staging(&intent)?;
        inventory::request(
            &json!({"backend":self.backend,"directory":self.directory,"inputs":intent["inputs"]}),
            true,
        )?;
        let producer = self.read("source-producer.json")?;
        let configuration =
            common::bound_file(self.backend, &intent["configuration"], "生产者配置")?;
        if configuration != self.directory.join("source-producer-configuration.json") {
            return Err("来源原生配置绑定不属于同一 verification 目录".into());
        }
        let expected = crate::recovery::clone::native::s3::app_configuration::configuration_value(
            self.execution,
            &self.execution.join("config"),
            "test",
        )?;
        let command = self.policy("producer-command", json!({"staging":staged["root"],"backend":self.backend,
            "execution":self.execution,"directory":self.directory,"operation":intent["operation_id"],
            "tool":self.tool,"start":self.start,"lineage":self.lineage,"configuration":configuration}))?;
        let value = self.policy("producer-registration", json!({"backend":self.backend,"directory":self.directory,
            "start":self.start,"lineage":self.lineage,"tool":self.tool,"argv":command["argv"],
            "environment_sha256":digest::plan_hash(&json!(self.environment))?,"coordinator_source":self.coordinator,
            "execution_sha":self.execution_sha,"configuration":expected}))?;
        if value != json!({"intent":intent,"producer":producer}) {
            return Err("来源验收生产者在登记复核期间变化".into());
        }
        self.source_unchanged()?;
        self.staging(&intent)?;
        Ok(value)
    }

    pub(super) fn verify(&self) -> Result<Value> {
        let registered = self.registered()?;
        self.ready(&registered["intent"])?;
        let completion = self.completion(&registered)?;
        let stderr = self.directory.join("source-producer.stderr.log");
        common::common_file(self.backend, &stderr, "生产者 stderr")?;
        if completion["exit_code"].as_i64() != Some(0) || std::fs::metadata(stderr)?.len() != 0 {
            return Err("来源验收生产者退出状态或 stderr 不可信".into());
        }
        self.stopped(&registered["producer"])?;
        let path = self.directory.join("source-producer.stdout.json");
        let before = common::binding(&path)?;
        let value =
            documents::read(&path).map_err(|_| "来源验收生产者 stdout 不是严格 JSON 对象")?;
        document::object(&value, "来源验收生产者结果")?;
        if common::binding(&path)? != before
            || self.registered()? != registered
            || self.completion(&registered)? != completion
        {
            return Err("来源验收生产者完成证据在读取期间变化".into());
        }
        self.stopped(&registered["producer"])?;
        Ok(json!({"producer":registered["producer"],"stdout":value}))
    }

    pub(super) fn ready(&self, intent: &Value) -> Result<Value> {
        let path = self.directory.join("source-producer-ready.json");
        common::common_file(self.backend, &path, "来源生产者 ready")?;
        let binding = common::binding(&path)?;
        let mut raw = Vec::new();
        File::open(&path)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut raw)?;
        let raw = std::str::from_utf8(&raw).map_err(|_| "来源生产者 ready 不是 UTF-8")?;
        let value = ready::verify(&json!({"raw":raw,"operation":intent["operation_id"],
            "start":self.start,"lineage":self.lineage,"staging":self.staging(intent)?}))?;
        if common::binding(&path)? != binding {
            return Err("来源生产者 ready 在读取期间变化".into());
        }
        Ok(value)
    }

    pub(super) fn completion(&self, registered: &Value) -> Result<Value> {
        self.policy(
            "producer-completion",
            json!({"backend":self.backend,"directory":self.directory,
            "intent":registered["intent"],"producer":registered["producer"]}),
        )
    }

    pub(super) fn stopped(&self, producer: &Value) -> Result<()> {
        let identity = common::validate_process_identity(&producer["process"])?;
        let pid = identity["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or("来源生产者 PID 超出内核范围")?;
        if common::process_identity(pid)?.as_ref() == Some(&identity) {
            return Err("来源验收原生生产者仍在运行，generation recover 必须失败关闭".into());
        }
        Ok(())
    }

    fn read(&self, name: &str) -> Result<Value> {
        common::read_bound(
            self.backend,
            &common::binding(&self.directory.join(name))?,
            "来源验收生产者证据",
        )
    }
}
