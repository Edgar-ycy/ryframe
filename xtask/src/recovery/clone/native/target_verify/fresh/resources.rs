//! 初始化前的真实资源观察；只运行固定读命令，不创建数据库、对象或缓存标记。
use super::super::super::{
    bridge::scope::Scope, export::transport::Transport, json as document, resources::common,
};
use super::super::{
    authority,
    input::{request, request_physical},
    storage::{self, StorageInputs, SystemObserver},
};
use super::{receipts, selection};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::clone::native) struct Resources<'a, T> {
    pub(in crate::recovery::clone::native) inputs: StorageInputs<'a>,
    pub(in crate::recovery::clone::native) transport: &'a T,
    pub(in crate::recovery::clone::native) scope: &'a Scope,
    pub(in crate::recovery::clone::native) run: Option<&'a Path>,
}

impl<T: Transport> Resources<'_, T> {
    pub(super) fn generation(&self) -> Result<Value> {
        Ok(self.generation_owned(None, true)?.0)
    }

    pub(in crate::recovery::clone::native) fn generation_owned(
        &self,
        owner: Option<&super::super::guard::Generation>,
        publish: bool,
    ) -> Result<(Value, selection::Selected, Option<Value>)> {
        storage::redis_layout::require_windows(&self.inputs.request["storage"]["redis"])?;
        let mut generation = authority::generation::observe(
            self.inputs.backend,
            self.inputs.request,
            self.inputs.environment,
            &mut |root, path, value| self.scope.maintenance(root, path, value),
            &mut authority::require_closed,
        )?;
        let (selected, cache) =
            selection::transition(self.inputs.backend, self.inputs.request, self.run, owner)?;
        let mut descriptor =
            document::object(&self.inputs.request["review"], "fresh 审阅绑定")?.clone();
        descriptor.remove("canonical_sha256");
        let review = common::read_bound(
            self.inputs.backend,
            &Value::Object(descriptor),
            "fresh 审阅计划",
        )?;
        let observer = SystemObserver;
        let redis = &selected.value["redis"];
        storage::redis_layout::require_windows(redis)?;
        let rustfs = storage::rustfs::inspect(
            &self.inputs,
            &review,
            &selected.value["rustfs"],
            selected.runtime.as_ref(),
            &observer,
        )?;
        if publish {
            storage::rustfs::publish(&self.inputs, &rustfs)?;
        }
        storage::redis_layout::verify(&self.inputs, &review, redis, &observer)?;
        if publish {
            let path = self.inputs.observation_dir.join(format!(
                "redis-windows-{}.json",
                uuid::Uuid::new_v4().simple()
            ));
            receipts::publish(self.transport, &path, redis)?;
        }
        storage::kernel::require_observed_identity(
            &selected.value["rustfs"]["identity"],
            &observer,
        )?;
        generation["storage"] = selected.value.clone();
        Ok((generation, selected, cache))
    }

    pub(super) fn absence(&self, redis: &Value) -> Result<Value> {
        storage::redis_layout::require_windows(redis)?;
        let databases = document::array(&self.inputs.request["target"]["databases"], "fresh 四库")?
            .iter()
            .map(|db| database_absence(&self.inputs, self.transport, db))
            .collect::<Result<Vec<_>>>()?;
        let objects = super::objects::absence(&self.inputs, self.transport)?;
        let redis =
            super::super::redis::absence_with_observer(&self.inputs, redis, &SystemObserver)?;
        Ok(json!({"databases":databases,"objects":objects,"redis":redis}))
    }
}

pub(crate) fn database_absence(
    inputs: &StorageInputs<'_>,
    transport: &impl Transport,
    database: &Value,
) -> Result<Value> {
    let name = document::text(&database["database"], "数据库名")?;
    let raw = mysql(
        inputs,
        transport,
        database,
        &format!(
            "SELECT @@server_uuid; SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{name}';"
        ),
    )?;
    if raw.trim().lines().collect::<Vec<_>>()
        != [document::text(&database["server_uuid"], "MySQL UUID")?]
    {
        return Err("目标 MySQL UUID或精确数据库不存在前像不同".into());
    }
    Ok(
        json!({"server_uuid":database["server_uuid"],"database":name,"exists":false,"empty_checked":false}),
    )
}

pub(super) fn mysql(
    inputs: &StorageInputs<'_>,
    transport: &impl Transport,
    database: &Value,
    sql: &str,
) -> Result<String> {
    let before = request_physical::defaults_connection(inputs.backend, database)?;
    let name = document::text(&database["database"], "数据库名")?;
    if name.is_empty() || !name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_') {
        return Err("数据库名称不是已审阅的安全标识符".into());
    }
    let tool = request::external_file(&inputs.request["tools"]["mysql"])?;
    let args = vec![
        format!(
            "--defaults-file={}",
            document::text(&database["defaults_file"], "MySQL 私有配置")?
        ),
        "--connect-timeout=5".into(),
        "--batch".into(),
        "--raw".into(),
        "--skip-column-names".into(),
        "--binary-mode".into(),
        "--default-character-set=utf8mb4".into(),
    ];
    let raw = receipts::command(
        inputs,
        transport,
        tool,
        args,
        Some(sql.as_bytes().to_vec()),
        &format!("mysql-{}", document::text(&database["key"], "数据库 key")?),
    )?;
    if request_physical::defaults_connection(inputs.backend, database)? != before {
        return Err("目标MySQL私有配置在命令期间变化".into());
    }
    request::external_file(&inputs.request["tools"]["mysql"])?;
    Ok(raw)
}
