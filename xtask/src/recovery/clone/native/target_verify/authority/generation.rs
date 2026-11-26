//! 重算当前文件配置、维护工具和运行前置；不连接或修改业务资源。
use super::super::super::{json as document, lifecycle::manifest};
use super::super::{
    binding,
    input::{request, request_configuration},
};
use crate::Result;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(in crate::recovery::clone::native) enum PortWindow<'a> {
    Closed,
    /// 仅已独立核验进程树及监听归属的 API 可以使用此窗口。
    LiveApi(&'a str),
}

pub(crate) fn observe(
    backend: &Path,
    target: &Value,
    environment: &BTreeMap<String, String>,
    maintenance: &mut dyn FnMut(&Path, &Path, &Value) -> Result<Value>,
    closed_port: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Value> {
    observe_in_window(
        backend,
        target,
        environment,
        maintenance,
        closed_port,
        PortWindow::Closed,
    )
}

pub(in crate::recovery::clone::native) fn observe_in_window(
    backend: &Path,
    target: &Value,
    environment: &BTreeMap<String, String>,
    maintenance: &mut dyn FnMut(&Path, &Path, &Value) -> Result<Value>,
    closed_port: &mut dyn FnMut(&str) -> Result<()>,
    window: PortWindow<'_>,
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
    require_ports(&selected.selected, window, closed_port)?;
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

fn require_ports(
    selected: &Value,
    window: PortWindow<'_>,
    closed_port: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    let api = document::text(&selected["api_url"], "目标 API 地址")?;
    match window {
        PortWindow::Closed => closed_port(api)?,
        PortWindow::LiveApi(verified) if api == verified => (),
        PortWindow::LiveApi(_) => return Err("审阅的目标 API 地址不同于已核验的原 API 地址".into()),
    }
    for field in ["worker_ready_url", "frontend_url"] {
        closed_port(document::text(&selected[field], "目标关闭端口")?)?;
    }
    Ok(())
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

#[cfg(test)]
#[test]
fn api_window_requires_the_exact_selected_url_and_other_roles_remain_closed() -> Result<()> {
    let api = "http://127.0.0.1:19010";
    let mut selected = json!({
        "api_url": api,
        "worker_ready_url": "http://127.0.0.1:19011/readyz",
        "frontend_url": "http://127.0.0.1:19012",
    });
    let mut checked = Vec::new();
    let mut record = |url: &str| {
        checked.push(url.to_owned());
        Ok(())
    };
    require_ports(&selected, PortWindow::Closed, &mut record)?;
    assert_eq!(
        checked,
        [
            api,
            "http://127.0.0.1:19011/readyz",
            "http://127.0.0.1:19012",
        ]
    );
    let still_open = &mut |url: &str| {
        if url == api {
            return Err("仍在监听".into());
        }
        Ok(())
    };
    require_ports(&selected, PortWindow::LiveApi(api), still_open)?;
    assert!(
        require_ports(
            &selected,
            PortWindow::LiveApi("http://127.0.0.1:19010/readyz"),
            &mut |_| Ok(()),
        )
        .is_err()
    );
    for field in ["worker_ready_url", "frontend_url"] {
        let original = std::mem::replace(&mut selected[field], json!(api));
        assert!(require_ports(&selected, PortWindow::LiveApi(api), still_open).is_err());
        selected[field] = original;
    }
    Ok(())
}
