//! 重算当前文件配置、维护工具和运行前置；不连接或修改业务资源。
use super::super::super::{json as document, lifecycle::manifest};
use super::super::{
    binding,
    input::{request, request_configuration},
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn observe(
    backend: &Path,
    target: &Value,
    environment: &BTreeMap<String, String>,
    maintenance: &mut dyn FnMut(&Path, &Path, &Value) -> Result<Value>,
    closed_port: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Value> {
    let selected = request::validate(backend, target)?;
    binding::never_started(backend, &selected.selected)?;
    let mut generation = request_configuration::validate(backend, target, &selected, environment)?;
    require_addresses(&generation["physical"], &selected.selected)?;
    let path = manifest::bound_file(backend, &target["maintenance_build"], "目标维护构建", true)?;
    let receipt = document::read(&path)?;
    let actual = maintenance(&selected.execution_backend, &path, &receipt)?;
    if actual != receipt {
        return Err("目标当前维护工具观察与绑定构建不同".into());
    }
    for field in ["api_url", "worker_ready_url", "frontend_url"] {
        closed_port(document::text(&selected.selected[field], "目标关闭端口")?)?;
    }
    binding::never_started(backend, &selected.selected)?;
    let after = maintenance(&selected.execution_backend, &path, &receipt)?;
    if request_configuration::validate(backend, target, &selected, environment)? != generation
        || request::validate(backend, target)?.execution != selected.execution
        || manifest::bound_file(backend, &target["maintenance_build"], "目标维护构建", true)?
            != path
        || document::read(&path)? != receipt
        || after != actual
    {
        return Err("目标代次观察期间配置、执行来源或维护构建变化".into());
    }
    generation["maintenance"] = actual;
    generation["operator_declared_controlled_generation_only"] = json!(true);
    generation["external_writers_discovered"] = json!(false);
    Ok(generation)
}

fn require_addresses(physical: &Value, selected: &Value) -> Result<()> {
    let planned = document::array(&selected["databases"], "审阅数据库")?;
    for actual in document::array(&physical["databases"], "实际数据库配置")? {
        let expected = planned
            .iter()
            .find(|row| row["key"] == actual["key"])
            .ok_or("物理数据库不属于审阅计划")?;
        if ["host", "port"]
            .iter()
            .any(|field| actual[*field] != expected[*field])
        {
            return Err("目标MySQL host/port不是审阅计划中的地址".into());
        }
    }
    Ok(())
}
