//! 存储旧调用方共用的只读证据端口；不接收命令或明文凭据。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use super::{StorageInputs, SystemObserver, common, generation, json_io, redis_layout, rustfs};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, export::transport::SystemTransport, lifecycle::manifest},
        fixture_control::services::lifecycle::readonly::process,
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

fn layout(backend: &Path, target: &Path, value: &Value) -> Result<Value> {
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
    let transport = SystemTransport;
    let observer = SystemObserver::new(&transport);
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
