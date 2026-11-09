//! 存储旧调用方共用的只读证据端口；不接收命令或明文凭据。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use super::{StorageInputs, SystemObserver, common, generation, json_io, redis_layout, rustfs};
use crate::{
    Result,
    recovery::{
        clone::native::{
            digest,
            lifecycle::{manifest, state},
        },
        fixture_control::services::{history, lifecycle::readonly::process},
    },
};

pub(crate) fn execute(
    backend: &Path,
    target: &Path,
    operation: &str,
    value: &Value,
) -> Result<Value> {
    match operation {
        "process-argv" => arguments(backend, target, &value["receipt"], &value["identity"]),
        "storage-generation" => {
            generation::validate_complete(
                &value["expected"],
                &value["actual"],
                optional(value, "runtime")?,
                optional(value, "cache_runtime")?,
            )?;
            Ok(json!({"status":"storage_generation_verified","remote_writes":0}))
        }
        "storage-layout" => layout(backend, target, value),
        "fixture-storage-selection" => fixture_selection(backend, value),
        "fixture-storage-layout" => fixture_layout(backend, target, value),
        "redis-initialized-state" => super::namespace::initialized(backend, target, value),
        _ => Err("未知存储证据操作".into()),
    }
}

/// 只返回摘要；即使 CIM 返回了意外进程的命令，也不会将参数或凭据写入响应。
pub(crate) fn arguments(
    backend: &Path,
    target: &Path,
    receipt: &Value,
    expected: &Value,
) -> Result<Value> {
    let path = common::bound_file(backend, receipt, "进程参数收据")?;
    if path.parent() != Some(target) {
        return Err("参数收据不属于明确登记的运行目录".into());
    }
    let document = json_io::read(&path)?;
    let identity = document.get("identity").ok_or("参数收据缺少创建身份")?;
    if identity != expected {
        return Err("参数收据与本次完整创建身份不同".into());
    }
    common::validate_process_identity(identity)?;
    let arguments = process::arguments(identity)?;
    common::bound_file(backend, receipt, "进程参数收据")?;
    Ok(
        json!({"actual_arguments_sha256":digest::plan_hash(&json!(arguments))?,
        "remote_writes":0}),
    )
}

fn fixture_selection(backend: &Path, value: &Value) -> Result<Value> {
    let descriptor = &value["request"];
    let run = Path::new(json_io::text(&value["storage_run"], "夹具服务运行目录")?);
    let before = fixture_registration(backend, run)?;
    let request = common::read_bound(backend, descriptor, "存储布局请求")?;
    let (selected, cache) =
        super::super::fresh::selection::transition(backend, &request, Some(run), None)?;
    let initial = selected.runtime.is_none() && cache.is_none();
    let projection = json!({
        "storage": selected.value,
        "runtime": selected.runtime,
        "cache_runtime": cache,
    });
    if initial && fixture_projection(backend, run, &request["storage"])? != projection {
        return Err("存储布局请求不属于原夹具服务代次".into());
    }
    if fixture_registration(backend, run)? != before
        || common::read_bound(backend, descriptor, "存储布局请求")? != request
    {
        return Err("夹具存储选择期间原请求或服务登记变化".into());
    }
    Ok(json!({
        "storage": projection["storage"],
        "runtime": projection["runtime"],
        "cache_runtime": projection["cache_runtime"],
        "manifest": before[0],
        "state": before[1],
    }))
}

fn fixture_layout(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
    let descriptor = &value["request"];
    let run = Path::new(json_io::text(&value["storage_run"], "夹具服务运行目录")?);
    let selection = &value["selection"];
    json_io::exact(
        selection,
        &["storage", "runtime", "cache_runtime", "manifest", "state"],
        "夹具存储选择",
    )?;
    let before = fixture_registration(backend, run)?;
    if before[0] != selection["manifest"] || before[1] != selection["state"] {
        return Err("夹具存储选择与当前清单或账本不同".into());
    }
    let request = common::read_bound(backend, descriptor, "存储布局请求")?;
    let projection = fixture_projection(backend, run, &request["storage"])?;
    if ["storage", "runtime", "cache_runtime"]
        .iter()
        .any(|key| selection[*key] != projection[*key])
    {
        return Err("夹具存储选择与已登记代次投影不同".into());
    }
    layout(
        backend,
        target,
        &json!({
            "request": descriptor,
            "storage": projection["storage"],
            "runtime": projection["runtime"],
            "cache_runtime": projection["cache_runtime"],
        }),
    )?;
    // 终检复用历史证据校验，不再解析工具续签或观察实际服务。
    history::validate(run, &state::load(run, true)?, true, true)?;
    if fixture_registration(backend, run)? != before
        || common::read_bound(backend, descriptor, "存储布局请求")? != request
    {
        return Err("夹具存储布局观察期间原请求或服务登记变化".into());
    }
    Ok(json!({"status":"fixture_storage_layout_verified","remote_writes":0}))
}

/// 只从已核验的服务账本派生代次；调用方负责活体观察前后的清单和账本窗口。
pub(super) fn fixture_projection(backend: &Path, run: &Path, expected: &Value) -> Result<Value> {
    use super::super::resume_runtime::{fixture_evidence, storage_projection};

    fixture_manifest(backend, run)?;
    json_io::exact(expected, &["rustfs", "redis"], "原夹具存储布局")?;
    let state = state::load(run, true)?;
    let history = history::validate(run, &state, true, true)?;
    let generation = &history["active_generation"];
    if generation["kind"] == "initial" {
        let rustfs = common::read_bound(backend, &generation["rustfs"], "RustFS 首代结果")?;
        let redis = common::read_bound(backend, &generation["redis"], "Redis 首代结果")?;
        let origin =
            json!({"rustfs":storage_projection(&rustfs),"redis":redis["runtime"]["redis"]});
        if origin != *expected {
            return Err("存储布局请求不属于原夹具服务代次".into());
        }
        return Ok(json!({"storage":origin,"runtime":null,"cache_runtime":null}));
    }
    let path = common::bound_file(backend, generation, "当前夹具服务结果")?;
    if path.parent() != Some(run.join("results").as_path()) {
        return Err("当前夹具服务结果不属于本运行账本".into());
    }
    let (runtime, cache) = fixture_evidence(backend, expected, generation)?;
    Ok(json!({
        "storage":{"rustfs":runtime["storage"],"redis":cache["redis"]},
        "runtime":runtime,
        "cache_runtime":cache,
    }))
}

fn fixture_registration(backend: &Path, run: &Path) -> Result<[Value; 2]> {
    let manifest = fixture_manifest(backend, run)?;
    let state = common::common_file(backend, &run.join("state.json"), "夹具服务账本")?;
    Ok([manifest, common::binding(&state)?])
}

fn fixture_manifest(backend: &Path, run: &Path) -> Result<Value> {
    let run = common::local_directory(backend, run, "夹具服务运行目录")?;
    let manifest = common::common_file(backend, &run.join("manifest.json"), "夹具服务清单")?;
    let descriptor = common::binding(&manifest)?;
    history::validate_manifest(&common::read_bound(backend, &descriptor, "夹具服务清单")?)?;
    Ok(descriptor)
}

fn layout(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
    redis_layout::require_windows(&value["storage"]["redis"])?;
    let descriptor = &value["request"];
    let path = common::bound_file(backend, descriptor, "存储布局请求")?;
    let request = json_io::read(&path)?;
    let review = super::super::input::request_review::read(backend, &request["review"])?;
    let environment = BTreeMap::from([("APP_ENV".to_owned(), "test".to_owned())]);
    let inputs = StorageInputs {
        backend,
        request: &request,
        environment: &environment,
        observation_dir: target,
    };
    let root = inputs.execution_backend()?;
    let (directory, files) =
        super::super::input::request_configuration::fresh_files(&root, &environment, &request)?;
    if super::super::input::request_configuration::configuration_identity(&directory, &files)?
        != json_io::text(&request["configuration_sha256"], "布局配置摘要")?
    {
        return Err("存储布局配置文件与绑定摘要不同".into());
    }
    let table = crate::rate_limit::load_app_table(&root, Some(&directory), "test")
        .map_err(|_| "存储布局无法读取绑定 TOML 配置")?;
    for field in ["endpoint", "region", "access_key", "secret_key"] {
        let configured = table
            .get("object_storage")
            .and_then(|table| table.get(field))
            .and_then(toml::Value::as_str)
            .filter(|text| !text.is_empty())
            .ok_or("存储布局缺少明确对象配置字段")?;
        if request["target"]["s3"][field].as_str() != Some(configured) {
            return Err("存储布局请求与绑定对象配置不同".into());
        }
    }
    let runtime = optional(value, "runtime")?;
    let cache = optional(value, "cache_runtime")?;
    json_io::exact(&request["storage"], &["rustfs", "redis"], "原存储布局")?;
    json_io::exact(&value["storage"], &["rustfs", "redis"], "当前存储布局")?;
    generation::validate_transition(&request["storage"], &value["storage"], runtime, cache)?;
    validate_runtime_files(backend, runtime)?;
    validate_runtime_files(backend, cache)?;
    let observer = SystemObserver;
    let storage = &value["storage"];
    rustfs::verify(&inputs, &review, &storage["rustfs"], runtime, &observer)?;
    redis_layout::verify(&inputs, &review, &storage["redis"], &observer)?;
    process::require(&storage["rustfs"]["identity"])?;
    if super::super::input::request_configuration::fresh_files(&root, &environment, &request)?
        != (directory, files)
    {
        return Err("存储布局观察期间配置变化".into());
    }
    common::bound_file(backend, descriptor, "存储布局请求")?;
    super::super::input::request_review::read(backend, &request["review"])?;
    validate_runtime_files(backend, runtime)?;
    validate_runtime_files(backend, cache)?;
    Ok(json!({"status":"storage_layout_verified","remote_writes":0}))
}

fn optional<'a>(value: &'a Value, field: &str) -> Result<Option<&'a Value>> {
    match &value[field] {
        Value::Null => Ok(None),
        value @ Value::Object(_) => Ok(Some(value)),
        _ => Err("存储运行代次必须为对象或 null".into()),
    }
}

fn validate_runtime_files(backend: &Path, runtime: Option<&Value>) -> Result<()> {
    let Some(runtime) = runtime else {
        return Ok(());
    };
    for field in [
        "generation",
        "request",
        "restart_result",
        "ready",
        "runtime",
        "owner",
        "controller",
        "tools",
    ] {
        if let Some(binding) = runtime.get(field).filter(|binding| !binding.is_null()) {
            manifest::bound_file(backend, binding, "存储运行代次证据", true)?;
        }
    }
    Ok(())
}
