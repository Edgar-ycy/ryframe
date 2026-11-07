//! 两个参考只读入口共享观察；不授予远端写入或恢复权限。
use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use super::{files, inputs, native, path, protocol, text};
use crate::{Result, recovery::fixture_control::services::lifecycle::readonly::NativeRuntime};
use native::{export::transport::SystemTransport, resources::common};

mod binding;
mod collector;
pub(crate) mod defaults;

pub(super) fn execute(args: &Value) -> Result<Value> {
    super::exact(args, &["raw"])?;
    let request = protocol::request(args)?;
    run(&request)
}

pub(super) fn run(request: &Value) -> Result<Value> {
    let backend = path(&request["backend_dir"])?;
    let plan_path = path(&request["plan"])?;
    let plan = native::json::read(plan_path)?;
    let command = text(&request["command"])?;
    let side = match command {
        "check-dataset" => "source",
        "check-existing" => text(&request["side"])?,
        _ => return Err("只读参考执行只允许 check-dataset/check-existing".into()),
    };
    inputs::domain::validate_plan(&plan, backend)?;
    let work = files::work(&json!({"plan":plan}), false)?;
    let bound = binding::Inputs::capture(backend, plan_path, &plan, path(&work)?, side)?;
    let tools = collector::Collector {
        inputs: &bound,
        transport: &SystemTransport,
    };
    let mut runtime = NativeRuntime::new(backend);
    observe(
        &plan,
        side,
        command == "check-dataset",
        &tools,
        |identity, url| runtime.observe_listener(identity, url),
    )?;
    bound.unchanged()?;
    Ok(json!({"plan_sha256":native::digest::plan_hash(&plan)?,
        "side":side,"scope_id":plan[side]["scope_id"]}))
}

fn observe<T: native::export::transport::Transport>(
    plan: &Value,
    side: &str,
    empty: bool,
    tools: &collector::Collector<'_, T>,
    listener: impl FnOnce(&Value, &str) -> Result<()>,
) -> Result<()> {
    for database in plan[side]["databases"].as_array().ok_or("数据库列表缺失")? {
        verify_database(plan, side, database, tools)?;
    }
    tools.inputs.unchanged()?;
    let receipt = native::json::read(&tools.inputs.registration)?;
    if receipt["format_version"] != 1
        || receipt["role"] != "api"
        || receipt["scope_id"] != plan[side]["scope_id"]
        || !receipt["identity"].is_object()
    {
        return Err("测试进程收据的角色或隔离 scope 不匹配".into());
    }
    listener(&receipt["identity"], text(&plan[side]["api_url"])?)?;
    tools.inputs.unchanged()?;
    if empty {
        let database = plan["source"]["databases"]
            .as_array()
            .ok_or("源数据库缺失")?
            .iter()
            .find(|db| db["kind"] == "combined")
            .ok_or("源 combined 库缺失")?;
        if tools.count(database)? != "0" {
            return Err("source 已包含非 system 租户；禁止准备数据集".into());
        }
    }
    Ok(())
}

fn verify_database<T: native::export::transport::Transport>(
    plan: &Value,
    side: &str,
    database: &Value,
    tools: &collector::Collector<'_, T>,
) -> Result<()> {
    let identity = tools.query(database, "identity")?;
    if identity.split('\t').collect::<Vec<_>>()
        != [
            text(&database["server_uuid"])?,
            text(&database["database"])?,
        ]
    {
        return Err("数据库服务器 UUID 或物理库名与计划不同".into());
    }
    let response = tools.query(database, "ownership")?;
    let actual = response
        .lines()
        .map(|row| row.split('\t').map(str::to_owned).collect::<Vec<_>>())
        .collect::<BTreeSet<_>>();
    let kinds = if database["kind"] == "combined" {
        vec!["control", "tenant-data"]
    } else {
        vec!["tenant-data"]
    };
    let scope = text(&plan[side]["scope_id"])?;
    let expected = kinds
        .into_iter()
        .map(|kind| {
            vec![
                kind.to_owned(),
                scope.to_owned(),
                format!("ryframe-owner:v1:{scope}:{kind}"),
            ]
        })
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("数据库 ownership 与计划 scope、种类或 marker 不同".into());
    }
    Ok(())
}

fn bound_image(path: &Path) -> Result<Value> {
    super::unlinked(path)?;
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let (device, inode) = common::device_inode_for(path, &metadata)?;
    Ok(json!({"binding":common::binding(path)?,"device":device,"inode":inode}))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/source/evidence/reference_workflow/readonly/tests.rs"]
mod tests;
