//! 使用原冻结运行合同和生产者历史控制 seed 的 API 与 Worker。

use crate::{
    Result,
    recovery::{
        clone::native::{
            cache::handoff::history::Producer, export::proof, json as document,
            post_process::runtime as runtime_contract, resources::common,
        },
        full_stack::worker::{controller, role::Role},
    },
};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[path = "runtime_pair/process.rs"]
mod process;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/runtime_pair.rs"]
mod tests;

const TIMEOUT: f64 = 60.0;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StopKind {
    Explicit,
    StartupRollback,
}

impl StopKind {
    pub(super) fn publish(
        self,
        producer: &Producer<'_>,
        identity: &Value,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Explicit => producer.stopped(identity, check),
            Self::StartupRollback => producer.rollback(identity, check),
        }
    }
}

pub(super) type StopRecord<'a> = dyn Fn(StopKind, Role, &str, &Value, &str) -> Result<()> + 'a;

pub(super) struct Inputs<'a> {
    pub(super) backend: &'a Path,
    pub(super) product: &'a Path,
    pub(super) runtime: &'a Path,
    pub(super) contract: &'a Value,
    pub(super) environment: &'a Value,
    pub(super) api_url: &'a str,
}

pub(super) fn control(
    inputs: Inputs<'_>,
    operation: &str,
    roles: &[Role],
    checkpoint: &dyn Fn() -> Result<()>,
    record_start: &dyn Fn(Role, &str) -> Result<()>,
    record_stop: &StopRecord<'_>,
) -> Result<Value> {
    let roles = ordered(roles)?;
    if !matches!(operation, "start" | "stop" | "status") {
        return Err("seed 运行控制仅接受启动、停止或状态观察".into());
    }
    checkpoint()?;
    let pair = Pair {
        api_ready: format!("{}/readyz", inputs.api_url.trim_end_matches('/')),
        inputs,
    };
    pair.check_contract()?;
    let processes = match operation {
        "start" => pair.start(roles, checkpoint, record_start, record_stop)?,
        "stop" => pair.stop(roles, checkpoint, record_stop)?,
        _ => pair.status(roles, checkpoint)?,
    };
    pair.check_contract()?;
    let history = pair.history_path()?;
    checkpoint()?;
    Ok(summary(
        operation,
        pair.inputs.contract,
        pair.inputs.runtime,
        processes,
        history,
    ))
}

struct Pair<'a> {
    inputs: Inputs<'a>,
    api_ready: String,
}

impl Pair<'_> {
    fn check_contract(&self) -> Result<()> {
        let inputs = &self.inputs;
        if runtime_contract::product_root(inputs.backend, inputs.contract)? != inputs.product
            || runtime_contract::contract(inputs.backend, inputs.product, inputs.runtime)?
                != *inputs.contract
        {
            return Err("seed 双角色运行合同或原产品根变化".into());
        }
        let api_dir = Path::new(document::text(
            &inputs.contract["api_config_dir"],
            "原 API 配置目录",
        )?);
        let table = crate::rate_limit::load_app_table(inputs.product, Some(api_dir), "test")
            .map_err(|_| "seed 原 API 文件配置无法核验")?;
        proof::verify_api_address(inputs.api_url, &serde_json::to_value(table)?)?;
        distinct_endpoints(&self.api_ready, self.ready_url(Role::Worker)?)
    }

    fn ready_url(&self, role: Role) -> Result<&str> {
        match role {
            Role::Api => Ok(&self.api_ready),
            Role::Worker => document::text(
                &self.inputs.contract["worker_ready_url"],
                "原 Worker 就绪地址",
            ),
            Role::Frontend => Err("seed 只控制 API 或 Worker".into()),
        }
    }

    fn producer<'b>(&'b self, role: Role, token: &'b str) -> Result<Producer<'b>> {
        Producer::new(
            self.inputs.backend,
            self.inputs.runtime,
            self.inputs.contract,
            role,
            token,
            self.ready_url(role)?,
        )
    }

    fn history_path(&self) -> Result<Option<std::path::PathBuf>> {
        let path = self.inputs.runtime.join("producer-history.json");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(_) => Ok(Some(common::common_file(
                self.inputs.backend,
                &path,
                "seed 生产者历史",
            )?)),
        }
    }

    fn active_history(&self) -> Result<Option<Value>> {
        let Some(path) = self.history_path()? else {
            return Ok(None);
        };
        let collected = super::source_registration::lineage::runtime::collect_for_product(
            self.inputs.backend,
            self.inputs.product,
            self.inputs.runtime,
        )?;
        let binding = common::binding(&path)?;
        if collected["history"] != binding {
            return Err("seed 生产者历史在完整读取期间变化".into());
        }
        Ok(Some(common::read_bound(
            self.inputs.backend,
            &binding,
            "seed 生产者历史",
        )?))
    }
}

fn ordered(roles: &[Role]) -> Result<&'static [Role]> {
    match roles {
        [Role::Api] => Ok(&[Role::Api]),
        [Role::Worker] => Ok(&[Role::Worker]),
        [Role::Api, Role::Worker] | [Role::Worker, Role::Api] => Ok(&[Role::Api, Role::Worker]),
        _ => Err("seed 角色必须明确为不重复的 API 或 Worker".into()),
    }
}

fn distinct_endpoints(api: &str, worker: &str) -> Result<()> {
    let api = url::Url::parse(api).map_err(|_| "seed API 就绪地址无效")?;
    let worker = url::Url::parse(worker).map_err(|_| "seed Worker 就绪地址无效")?;
    if api.host_str() == worker.host_str()
        && api.port_or_known_default() == worker.port_or_known_default()
    {
        return Err("seed API 和 Worker 就绪端点不能相同".into());
    }
    Ok(())
}

fn summary(
    operation: &str,
    contract: &Value,
    runtime: &Path,
    processes: Value,
    history: Option<std::path::PathBuf>,
) -> Value {
    json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":operation,
        "scope_id":contract["scope_id"],"runtime_directory":runtime,"processes":processes,
        "producer_history":history})
}
