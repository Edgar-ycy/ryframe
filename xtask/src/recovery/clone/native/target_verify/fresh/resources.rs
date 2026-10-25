//! 初始化前的真实资源观察；只运行固定读命令，不创建数据库、对象或缓存标记。
use super::super::super::{
    bridge::scope::Scope, export::transport::Transport, json as document, resources::common,
};
use super::super::{
    authority,
    input::{request, request_physical},
    storage::{self, StorageInputs, StorageObserver, SystemObserver},
};
use super::{receipts, selection};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) struct Resources<'a, T> {
    pub(super) inputs: StorageInputs<'a>,
    pub(super) transport: &'a T,
    pub(super) scope: &'a Scope,
    pub(super) run: Option<&'a Path>,
}

impl<T: Transport> Resources<'_, T> {
    pub(super) fn generation(&self) -> Result<Value> {
        let mut generation = authority::generation::observe(
            self.inputs.backend,
            self.inputs.request,
            self.inputs.environment,
            &mut |root, path, value| self.scope.maintenance(root, path, value),
            &mut authority::require_closed,
        )?;
        let selected = selection::load(self.inputs.backend, self.inputs.request, self.run)?;
        let mut descriptor =
            document::object(&self.inputs.request["review"], "fresh 审阅绑定")?.clone();
        descriptor.remove("canonical_sha256");
        let review = common::read_bound(
            self.inputs.backend,
            &Value::Object(descriptor),
            "fresh 审阅计划",
        )?;
        let observer = Observer {
            transport: self.transport,
        };
        storage::rustfs::verify(
            &self.inputs,
            &review,
            &selected.value["rustfs"],
            selected.runtime.as_ref(),
            &observer,
        )?;
        storage::redis_layout::verify(&self.inputs, &review, &selected.value["redis"], &observer)?;
        storage::kernel::require_observed_identity(
            &selected.value["rustfs"]["identity"],
            &observer,
        )?;
        generation["storage"] = selected.value;
        Ok(generation)
    }

    pub(super) fn absence(&self) -> Result<Value> {
        let databases = document::array(&self.inputs.request["target"]["databases"], "fresh 四库")?
            .iter()
            .map(|db| self.database(db))
            .collect::<Result<Vec<_>>>()?;
        let objects = super::objects::absence(&self.inputs, self.transport)?;
        let redis = self.redis_absence()?;
        Ok(json!({"databases":databases,"objects":objects,"redis":redis}))
    }

    fn database(&self, database: &Value) -> Result<Value> {
        let before = request_physical::defaults_connection(self.inputs.backend, database)?;
        let tool = request::external_file(&self.inputs.request["tools"]["mysql"])?;
        let defaults = document::text(&database["defaults_file"], "MySQL 私有配置")?;
        let name = document::text(&database["database"], "数据库名")?;
        // request::validate 已限定标识符；此处仍不允许 SQL 元字符进入固定查询。
        if name.is_empty() || !name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_') {
            return Err("数据库名称不是已审阅的安全标识符".into());
        }
        let args = vec![
            format!("--defaults-file={defaults}"),
            "--connect-timeout=5".into(),
            "--batch".into(),
            "--raw".into(),
            "--skip-column-names".into(),
            "--binary-mode".into(),
            "--default-character-set=utf8mb4".into(),
        ];
        let sql = format!(
            "SELECT @@server_uuid; SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{name}';"
        );
        let raw = receipts::command(
            &self.inputs,
            self.transport,
            tool,
            args,
            Some(sql.into_bytes()),
            &format!("mysql-{}", document::text(&database["key"], "数据库 key")?),
        )?;
        if raw.trim().lines().collect::<Vec<_>>()
            != [document::text(&database["server_uuid"], "MySQL UUID")?]
            || request_physical::defaults_connection(self.inputs.backend, database)? != before
        {
            return Err("目标 MySQL UUID、精确数据库不存在前像或私有配置不同".into());
        }
        request::external_file(&self.inputs.request["tools"]["mysql"])?;
        Ok(
            json!({"server_uuid":database["server_uuid"],"database":name,"exists":false,"empty_checked":false}),
        )
    }

    fn redis_absence(&self) -> Result<Value> {
        super::super::redis::absence_with_observer(
            &self.inputs,
            &self.inputs.request["storage"]["redis"],
            &Observer {
                transport: self.transport,
            },
        )
    }
}

struct Observer<'a, T> {
    transport: &'a T,
}

impl<T: Transport> StorageObserver for Observer<'_, T> {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        common::process_identity(pid)
    }
    fn arguments(&self, inputs: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>> {
        SystemObserver::new(self.transport).arguments(inputs, identity)
    }
    fn listener(&self, inputs: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()> {
        SystemObserver::new(self.transport).listener(inputs, identity, url)
    }
    fn wsl(&self, inputs: &StorageInputs<'_>, redis: &Value, args: &[String]) -> Result<String> {
        let program = request::external_file(&redis["wsl"])?;
        let mut arguments = vec![
            "--distribution".into(),
            document::text(&redis["distribution"], "Redis 发行版")?.into(),
            "--exec".into(),
        ];
        arguments.extend_from_slice(args);
        let result = receipts::command(
            inputs,
            self.transport,
            program,
            arguments,
            None,
            "redis-kernel",
        )?;
        request::external_file(&redis["wsl"])?;
        Ok(result.trim().to_owned())
    }
    fn redis(&self, inputs: &StorageInputs<'_>, redis: &Value, parts: &[&str]) -> Result<Value> {
        SystemObserver::new(self.transport).redis(inputs, redis, parts)
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/target_verify/fresh/ports.rs"]
mod tests;
