//! 四库采集直接复用正式双轮 collector；派生参数只用于读端口，不发布或改写原请求。
use super::{Resources, Transport, document, json};
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        target_verify::{database, storage::StorageInputs},
    },
};
use serde_json::Value;
use std::path::Path;

impl<T: Transport> Resources<'_, T> {
    pub(crate) fn capture_databases(
        &self,
        directory: &Path,
        selected: &Value,
        tools: &Value,
        maintenance: &Value,
        scope: &Scope,
    ) -> Result<Value> {
        (self.checkpoint)()?;
        if self.request["target"]["scope_id"] != selected["scope_id"]
            || self.request["target"]["databases"] != selected["databases"]
            || self.request["tools"]["mysql"] != tools["mysql"]
        {
            return Err("来源四库采集不能改变已登记 scope、物理库或 MySQL 工具".into());
        }
        // 原初始化请求保持不可变；明确执行根和当前维护构建来自父层已验证 generation。
        let mut port = self.request.clone();
        port["execution_backend"] = json!({"path":self.execution});
        port["config_dir"] = json!(super::super::generation_request::configuration::directory(
            self.backend,
            self.generation
        )?);
        port["maintenance_build"] = maintenance.clone();
        let inputs = StorageInputs {
            backend: self.backend,
            request: &port,
            environment: self.environment,
            observation_dir: directory,
        };
        let source = database::SourceBinding {
            selected,
            tools,
            maintenance,
            scope,
            configuration: (self.generation["format_version"].as_u64() == Some(2))
                .then_some(&self.generation["configuration"]),
        };
        let checkpoint = || (self.checkpoint)();
        let captured =
            database::capture_source_at(&inputs, directory, self.transport, &checkpoint, &source)?;
        (self.checkpoint)()?;
        let verified = super::super::images::inventory::verify(
            self.backend,
            &captured.receipt,
            selected,
            self.execution,
        )?;
        document::object(&verified["databases"], "来源四库原始像")?;
        Ok(verified)
    }
}
