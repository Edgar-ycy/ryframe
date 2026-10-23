//! seed 运行交接和精确停止证据；停止证明不等于业务排空。
use super::{Registration, history, latest};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Inputs {
    pub(crate) runtime: PathBuf,
    pub(crate) handoff: Value,
    pub(crate) environment: Value,
}

pub(crate) fn inputs(backend: &Path, run: &Path) -> Result<Inputs> {
    let registered = Registration::load(backend, run, true)?;
    let post = history::post(
        backend,
        run,
        Some(&registered.value["post_copy"]),
        None,
        true,
    )?;
    let runtime = run.join("seed-runtime").join("runtime");
    validate_local_test_path(&runtime, backend, LocalTestPathKind::ExistingDirectory)?;
    let handoff_path = runtime.join("handoff.json");
    let handoff_binding = common::binding(&handoff_path)?;
    let handoff = document::read(&handoff_path)?;
    for (field, path) in [
        ("registration", run.join("seed-runtime.json")),
        ("runtime", runtime.join("runtime.json")),
        ("binaries", runtime.join("binaries.json")),
    ] {
        if handoff[field] != common::binding(&path)? {
            return Err("seed runtime 文件不同于固定交接".into());
        }
    }
    let saved = state::load(run, false)?;
    let prepared = latest(&saved, "seed-runtime", &["prepare"], None, true)?;
    let result = common::read_bound(backend, &prepared["result"], "seed prepare 发布结果")?;
    if result["status"] != "seed_runtime_prepared" || result["handoff"] != handoff_binding {
        return Err("seed runtime 尚未通过外层准备收尾".into());
    }
    let private = common::read_bound(
        backend,
        &post.request["api_environment"],
        "seed API 环境登记",
    )?;
    document::exact(&private, &["environment"], "seed API 环境登记")?;
    let environment = private["environment"].clone();
    if environment != json!({"APP_ENV":"test"}) {
        return Err("seed API 环境只能选择 APP_ENV=test，其他配置必须使用绑定文件".into());
    }
    registered.assert_unchanged(backend, true)?;
    if common::binding(&handoff_path)? != handoff_binding || state::load(run, false)? != saved {
        return Err("seed 运行交接或阶段结果在读取期间变化".into());
    }
    Ok(Inputs {
        runtime,
        handoff,
        environment,
    })
}

pub(crate) fn stopped(runtime: &Path, handoff: &Value) -> Result<Value> {
    stopped_with(runtime, handoff, &mut common::process_identity)
}

pub(crate) fn stopped_with(
    runtime: &Path,
    handoff: &Value,
    inspect: &mut dyn FnMut(u32) -> Result<Option<Value>>,
) -> Result<Value> {
    let path = runtime.join("producer-history.json");
    let before = common::binding(&path)?;
    let history = document::read(&path)?;
    if history["kind"] != "devex-clone-producer-history"
        || history["scope_id"] != handoff["scope_id"]
        || history["runtime_directory"].as_str() != runtime.to_str()
        || history["format_version"].as_u64() != Some(1)
    {
        return Err("生产者历史不属于固定 seed runtime".into());
    }
    let events = document::array(&history["events"], "生产者历史事件")?;
    if events
        .iter()
        .enumerate()
        .any(|(index, row)| row["sequence"].as_u64() != Some(index as u64))
    {
        return Err("生产者历史不连续".into());
    }
    let intents = events
        .iter()
        .filter(|row| row["event"] == "start-intent")
        .collect::<Vec<_>>();
    let mut tokens = std::collections::BTreeSet::new();
    for intent in intents {
        let token = intent["token"].as_str().ok_or("启动意图缺少 token")?;
        if !tokens.insert(token) {
            return Err("生产者启动意图重复".into());
        }
        let completed = events
            .iter()
            .filter(|row| {
                row["token"] == intent["token"]
                    && matches!(row["event"].as_str(), Some("started" | "start-failed"))
                    && row["role"] == intent["role"]
                    && row["sequence"].as_u64() > intent["sequence"].as_u64()
            })
            .collect::<Vec<_>>();
        if completed.is_empty() {
            return Err("启动意图缺少实际身份或失败收尾，需核实未知进程".into());
        }
        for record in completed
            .into_iter()
            .filter(|row| row["event"] == "started")
        {
            require_ended(&record["identity"], inspect)?;
        }
    }
    let mut processes = serde_json::Map::new();
    for role in ["api", "worker"] {
        let path = runtime.join(format!("{role}.json"));
        let descriptor = match fs::symlink_metadata(&path) {
            Ok(_) => common::binding(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(_) => return Err("生产者进程收据无法核验".into()),
        };
        processes.insert(role.into(), descriptor);
    }
    if common::binding(&path)? != before {
        return Err("生产者历史在停止核验期间变化".into());
    }
    Ok(json!({"history":before,"processes":processes}))
}

fn require_ended(
    recorded: &Value,
    inspect: &mut dyn FnMut(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let recorded = common::validate_process_identity(recorded)?;
    let pid = u32::try_from(recorded["pid"].as_u64().ok_or("生产者 PID 无效")?)?;
    let Some(current) = inspect(pid)? else {
        return Ok(());
    };
    let current = common::validate_process_identity(&current)?;
    let time = |value: &Value| -> Result<u128> {
        value["started"]
            .as_str()
            .ok_or("进程代次缺失")?
            .parse()
            .map_err(|_| "进程代次无效".into())
    };
    if current["pid"] != recorded["pid"] || time(&current)? <= time(&recorded)? {
        return Err("历史生产者仍存在或 PID 代次无法证明更新".into());
    }
    Ok(())
}

pub(crate) fn restart_after_stop(
    backend: &Path,
    run: &Path,
    runtime: &Path,
    handoff: &Value,
    failed: &Value,
    current: u64,
) -> Result<()> {
    if failed["stage"] != "seed-runtime" || failed["mode"] != "start" {
        return Err("仅明确停止可解除失败 start 的运行状态，其余失败仍需对应核验".into());
    }
    let saved = state::load(run, true)?;
    if !state::attempts(&saved)?.contains(failed) || failed["status"] != "failed" {
        return Err("seed 重启必须引用原始失败阶段，不能重新构造失败身份".into());
    }
    let previous = latest(&saved, "seed-runtime", &["stop"], Some(current), false)?;
    let result = common::read_bound(backend, &previous["result"], "seed 停止阶段结果")?;
    let empty = json!({"state":"stopped","identity":null,"ready":false});
    if previous["number"].as_u64() <= failed["number"].as_u64()
        || result["restart_ready"] != true
        || result["handoff"] != common::binding(&runtime.join("handoff.json"))?
        || result["stopped_evidence"] != stopped(runtime, handoff)?
        || result["runtime"]["operation"] != "stop"
        || result["runtime"]["runtime_directory"].as_str() != runtime.to_str()
        || result["runtime"]["scope_id"] != handoff["scope_id"]
        || result["runtime"]["processes"] != json!({"api":empty,"worker":empty})
    {
        return Err("缺少失败之后同一 runtime 的精确双角色停止证明".into());
    }
    if state::load(run, true)? != saved {
        return Err("seed 重启授权阶段记录变化".into());
    }
    Ok(())
}
