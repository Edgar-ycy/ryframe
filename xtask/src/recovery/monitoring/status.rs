use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener},
    path::Path,
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use super::{
    contract::{
        BASE_NAMES, RUNTIME_DIRECTORIES, loopback_port, process_identity, read_binding,
        read_document, sorted_ports,
    },
    receipts::{process_trees, read_start, validate_close, validate_observation, validate_result},
};
use crate::{Result, recovery::clone::native::resources::common};

pub(crate) fn status(backend: &Path, binding_path: &Path) -> Result<Value> {
    let binding = read_binding(backend, binding_path)?;
    let names = super::contract::validate_root_shape(&binding.run)?;
    let mut state = "bound";
    let mut next_action = Some("start");
    let mut alive = Map::new();
    if names.contains("result.json") {
        let result = read_document(&binding.run.join("result.json"), "监控最终成功收据")?;
        validate_result(
            &result.value,
            &binding.value,
            &binding.document.descriptor(),
        )?;
        result.assert_unchanged()?;
        state = "passed";
        next_action = None;
    } else if names.contains("close-failure.json") {
        state = "close-failed";
        next_action = None;
    } else if names.contains("close.json") {
        state = "closed";
        next_action = names.contains("observation.json").then_some("result");
    } else if names.contains("close-intent.json") {
        state = "closing";
        next_action = Some("close");
    } else if names.contains("observation-failure.json") {
        state = "observation-failed";
        next_action = Some("close");
    } else if names.contains("observation.json") {
        state = "observed";
        next_action = Some("close");
    } else if names.contains("start-failure.json") {
        state = "start-failed";
        next_action = Some("close");
    } else if names.contains("start.json") {
        let (_, start, _, _) =
            read_start(&binding.run, &binding.value, &binding.document.descriptor())?;
        let processes = start["processes"]
            .as_object()
            .ok_or("监控启动收据缺少进程")?;
        for (role, item) in processes {
            let identity = item.get("identity").ok_or("监控进程缺少身份")?;
            let (pid, expected) = process_identity(identity, &format!("监控进程 {role}"))?;
            alive.insert(
                role.clone(),
                Value::Bool(common::process_identity(pid)?.as_ref() == Some(&expected)),
            );
        }
        let running = alive.values().all(|value| value.as_bool() == Some(true));
        state = if running { "running" } else { "interrupted" };
        next_action = Some(if running { "observe" } else { "close" });
    } else if names.contains("start-intent.json") {
        state = "starting-interrupted";
        next_action = Some("close");
    }
    binding.document.assert_unchanged()?;
    Ok(json!({
        "status": state,
        "run_id": binding.value["run_id"],
        "scope_id": binding.value["scope_id"],
        "next_action": next_action,
        "processes_alive": alive,
        "binding": binding.document.descriptor(),
        "external_contacts": 0,
    }))
}

pub(crate) fn result(backend: &Path, binding_path: &Path) -> Result<Value> {
    let binding = read_binding(backend, binding_path)?;
    let expected = BASE_NAMES
        .iter()
        .chain(RUNTIME_DIRECTORIES.iter())
        .chain(
            [
                "start-intent.json",
                "start.json",
                "observation.json",
                "close-intent.json",
                "close.json",
            ]
            .iter(),
        )
        .map(|value| (*value).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    if super::contract::validate_root_shape(&binding.run)? != expected {
        return Err("监控 run 不处于最终结果发布要求的精确状态".into());
    }
    let binding_receipt = binding.document.descriptor();
    let (start_document, _, _, operations) =
        read_start(&binding.run, &binding.value, &binding_receipt)?;
    let start_receipt = start_document.descriptor();
    let observation_document =
        read_document(&binding.run.join("observation.json"), "监控观测收据")?;
    let observation = validate_observation(
        &observation_document.value,
        &binding.value,
        &binding_receipt,
        &start_receipt,
    )?;
    let observation_receipt = observation_document.descriptor();
    let close_intent_document =
        read_document(&binding.run.join("close-intent.json"), "监控关闭意图")?;
    let close_intent_receipt = close_intent_document.descriptor();
    let close_document = read_document(&binding.run.join("close.json"), "监控关闭收据")?;
    let close = validate_close(
        &close_document.value,
        &binding.value,
        &binding_receipt,
        &observation_receipt,
        &close_intent_receipt,
        &operations,
    )?;
    if close["completions"]
        .as_object()
        .is_none_or(|value| value.len() != super::contract::ROLES.len())
    {
        return Err("监控成功结果缺少三棵完整进程树关闭证明".into());
    }
    super::contract::validate_runtime_layout(&binding.run, &operations, true)?;
    let scope = binding.value["scope_id"]
        .as_str()
        .ok_or("监控 scope_id 无效")?;
    let trees = process_trees(&binding.run, scope, &operations)?;
    for (role, identity) in trees {
        let (pid, expected) = process_identity(&identity, &format!("监控进程 {role}"))?;
        if common::process_identity(pid)?.as_ref() == Some(&expected) {
            return Err("监控进程仍在运行，不能发布最终结果".into());
        }
    }
    for endpoint in binding.value["endpoints"]
        .as_object()
        .into_iter()
        .flat_map(|value| value.values())
    {
        let url = endpoint.as_str().ok_or("监控端点无效")?;
        let port = loopback_port(url, "")?;
        require_closed(url, port)?;
    }
    let result = json!({
        "format_version": 1,
        "kind": "restore-monitoring-result",
        "status": "passed",
        "binding": binding_receipt,
        "start": start_receipt,
        "observation": observation_receipt,
        "close": close_document.descriptor(),
        "run_id": binding.value["run_id"],
        "scope_id": binding.value["scope_id"],
        "alerts": observation["prometheus"]["alerts"].as_array()
            .map(|values| {
                let mut values = values.clone();
                values.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
                values
            })
            .ok_or("监控告警证据无效")?,
        "targets": observation["prometheus"]["targets"].as_array()
            .map(|values| {
                let mut values = values.clone();
                values.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
                values
            })
            .ok_or("监控 target 证据无效")?,
        "firing_delivered": observation["delivery"]["firing"],
        "resolved_delivered": observation["delivery"]["resolved"],
        "boundary_cases": observation["boundaries"]["cases"],
        "ports_closed": sorted_ports(&binding.value),
        "external_contacts": 0,
        "completed_at": utc_now(),
    });
    validate_result(&result, &binding.value, &binding.document.descriptor())?;
    for document in [
        &start_document,
        &observation_document,
        &close_intent_document,
        &close_document,
        &binding.document,
    ] {
        document.assert_unchanged()?;
    }
    super::publish::terminal(&binding.run, &result)?;
    Ok(result)
}

fn require_closed(url: &str, port: u16) -> Result<()> {
    let address = if url.starts_with("http://[::1]:") {
        SocketAddr::from((Ipv6Addr::LOCALHOST, port))
    } else {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    };
    TcpListener::bind(address)
        .map(drop)
        .map_err(|error| format!("监控端口 {port} 仍被占用或无法核验：{error}").into())
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
