use std::{collections::BTreeMap, path::Path};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{
    bridge, digest,
    json::{self as json_io, array, exact, object, text},
    lifecycle::manifest,
    maintenance,
    resources::common,
    source,
};
use super::input::Context;
use crate::Result;

pub(crate) fn generation(context: &Context) -> Result<Value> {
    let build_path = manifest::bound_file(
        &context.backend,
        context.request.get("backend_build").unwrap(),
        "API/Worker 构建",
        true,
    )?;
    let build = json_io::read(&build_path)?;
    bridge::verify_build(&context.backend, &build_path, &build)?;
    let current = source::capture(&context.execution_backend)?;
    let expected = build
        .pointer("/sources/full/source")
        .cloned()
        .ok_or("API/Worker 构建缺少完整来源")?;
    if source::binding_value(&current.source) != expected
        || current.source.worktree_fingerprint
            != context
                .request
                .get("worktree_fingerprint")
                .and_then(Value::as_str)
                .unwrap_or_default()
    {
        return Err("实际源码与构建、工作区指纹或源请求不一致".into());
    }
    verify_maintenance(context, &expected)?;
    let runtime_path = manifest::bound_file(
        &context.backend,
        context.request.get("runtime").unwrap(),
        "源运行收据",
        true,
    )?;
    let runtime = json_io::read(&runtime_path)?;
    if runtime.get("scope_id").and_then(Value::as_str)
        != context.source().get("scope_id").and_then(Value::as_str)
    {
        return Err("源 runtime scope 与请求不一致".into());
    }
    let artifacts = object(
        build.get("artifacts").ok_or("API/Worker 构建缺少产物")?,
        "API/Worker 构建产物",
    )?;
    let runtime_artifacts = object(
        runtime.get("artifacts").ok_or("源 runtime 缺少产物")?,
        "源 runtime 产物",
    )?;
    let mut processes = BTreeMap::new();
    for role in ["api", "worker"] {
        let artifact = exact(
            artifacts.get(role).ok_or("API/Worker 构建缺少产物")?,
            &["executable", "command", "bytes", "sha256"],
            "API/Worker 构建产物",
        )?;
        let executable = text(artifact.get("executable").unwrap(), "产物路径")?;
        let expected_runtime = json!({
            "path": executable,
            "sha256": text(artifact.get("sha256").unwrap(), "产物摘要")?,
        });
        if runtime_artifacts.get(role) != Some(&expected_runtime) {
            return Err("源 runtime 产物与当前构建不一致".into());
        }
        let receipt_path = manifest::bound_file(
            &context.backend,
            context
                .request
                .pointer(&format!("/processes/{role}"))
                .ok_or("源请求缺少进程收据")?,
            "源进程收据",
            true,
        )?;
        let receipt_value = json_io::read(&receipt_path)?;
        let receipt = exact(
            &receipt_value,
            &["format_version", "role", "scope_id", "identity"],
            "源进程收据",
        )?;
        validate_identity(receipt.get("identity").unwrap(), "源进程身份")?;
        if receipt.get("format_version").and_then(Value::as_u64) != Some(1)
            || receipt.get("role").and_then(Value::as_str) != Some(role)
            || receipt.get("scope_id") != context.source().get("scope_id")
            || receipt_value
                .pointer("/identity/executable")
                .and_then(Value::as_str)
                != Some(executable)
        {
            return Err("源进程角色、scope 或实际二进制与构建不一致".into());
        }
        processes.insert(role.to_owned(), receipt["identity"].clone());
    }
    let producers = producers(context, &processes)?;
    if context.environment.get("APP_JOBS_MODE").map(String::as_str) != Some("external") {
        return Err("源必须明确使用 external Worker，scheduler/consumer 由 Worker 承载".into());
    }
    let api = context
        .source()
        .get("api_url")
        .and_then(Value::as_str)
        .ok_or("源缺少 API 地址")?;
    let worker = runtime
        .get("worker_ready_url")
        .and_then(Value::as_str)
        .ok_or("源 runtime 缺少 Worker 就绪地址")?;
    verify_api_address(&context.execution_backend, api, &context.environment)?;
    common::require_closed_port(api)?;
    common::require_closed_port(worker)?;
    Ok(json!({
        "source": expected["snapshot"],
        "worktree_fingerprint": context.request["worktree_fingerprint"],
        "runtime": runtime,
        "processes": processes,
        "operator_declared_producers": producers,
        "physical_binding": physical_binding(context)?,
        "maintenance": json_io::read(&manifest::bound_file(
            &context.execution_backend,
            context.request.get("maintenance_build").unwrap(),
            "维护构建",
            true,
        )?)?,
        "request_sha256": digest::plan_hash(&context.request)?,
        "external_writers_discovered": false,
        "clone_verified": false,
        "restore_qualified": false,
    }))
}

fn verify_maintenance(context: &Context, expected: &Value) -> Result<()> {
    let path = manifest::bound_file(
        &context.execution_backend,
        context.request.get("maintenance_build").unwrap(),
        "维护构建",
        true,
    )?;
    let receipt = maintenance::verify_evidence(&context.execution_backend, &path, None)?;
    if receipt.pointer("/source/snapshot") != expected.get("snapshot")
        || receipt.pointer("/source/worktree_fingerprint")
            != context.request.get("worktree_fingerprint")
    {
        return Err("维护工具与源实际后端构建不属于同一完整源码".into());
    }
    Ok(())
}

fn producers(context: &Context, processes: &BTreeMap<String, Value>) -> Result<Vec<Value>> {
    let path = manifest::bound_file(
        &context.backend,
        context.request.get("producers_registry").unwrap(),
        "producer 清单",
        true,
    )?;
    let registry = json_io::read(&path)?;
    let registry = exact(
        &registry,
        &["format_version", "scope_id", "runtime_sha256", "processes"],
        "producer 清单",
    )?;
    let runtime_sha = context
        .request
        .pointer("/runtime/sha256")
        .and_then(Value::as_str)
        .ok_or("源运行收据缺少摘要")?;
    let items = array(registry.get("processes").unwrap(), "producer 清单")?;
    if registry.get("format_version").and_then(Value::as_u64) != Some(1)
        || registry.get("scope_id") != context.source().get("scope_id")
        || registry.get("runtime_sha256").and_then(Value::as_str) != Some(runtime_sha)
        || items.len() < 3
    {
        return Err("操作人声明的 producer 清单不完整或 scope 不同".into());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let item = exact(item, &["name", "identity"], "producer")?;
        let name = text(item.get("name").unwrap(), "producer 名称")?;
        let identity = validate_identity(item.get("identity").unwrap(), "producer 身份")?;
        if !names.insert(name.to_owned()) {
            return Err("producer 名称重复".into());
        }
        if processes
            .get(name)
            .is_some_and(|recorded| recorded != identity)
        {
            return Err("producer 清单中的 API/Worker 代次与真实运行收据不同".into());
        }
        require_stopped(identity)?;
        result.push(Value::Object(item.clone()));
    }
    if !names.contains("api") || !names.contains("worker") {
        return Err("producer 清单缺少 API 或 Worker".into());
    }
    result.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    Ok(result)
}

fn require_stopped(identity: &Value) -> Result<()> {
    let pid = identity
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("producer PID 无效")?;
    let original = identity
        .get("started")
        .and_then(Value::as_str)
        .ok_or("producer 创建代次无效")?
        .parse::<u128>()
        .map_err(|_| "producer 创建代次无效")?;
    let Some(current) = common::process_identity(pid)? else {
        return Ok(());
    };
    validate_identity(&current, "当前 producer 身份")?;
    let started = current
        .get("started")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u128>().ok())
        .ok_or("当前 producer 创建代次无效")?;
    if started <= original {
        return Err("登记 producer 尚未停止或当前创建代次不明确".into());
    }
    Ok(())
}

fn validate_identity<'a>(value: &'a Value, label: &str) -> Result<&'a Value> {
    let original = value;
    let value = exact(original, &["pid", "started", "executable"], label)?;
    if value
        .get("pid")
        .and_then(Value::as_u64)
        .is_none_or(|value| value <= 1)
        || value
            .get("started")
            .and_then(Value::as_str)
            .is_none_or(|value| {
                !Regex::new(r"^[1-9][0-9]{0,17}$").is_ok_and(|re| re.is_match(value))
            })
        || value
            .get("executable")
            .and_then(Value::as_str)
            .is_none_or(|value| !Path::new(value).is_absolute())
    {
        return Err(format!("{label}无效").into());
    }
    Ok(original)
}

fn physical_binding(context: &Context) -> Result<Value> {
    let mut databases = Vec::new();
    for database in context.databases()? {
        databases.push(json!({
            "key": database["key"],
            "kind": database["kind"],
            "server_uuid": database["server_uuid"],
            "database": database["database"],
        }));
    }
    Ok(json!({
        "scope_id": context.source()["scope_id"],
        "object_endpoint": context.source()["s3"]["endpoint"],
        "databases": databases,
    }))
}

fn verify_api_address(
    backend: &Path,
    url: &str,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    if environment.contains_key("APP_APP_HOST_FILE")
        || environment.contains_key("APP_APP_PORT_FILE")
    {
        return Err("源 API 地址文件覆盖未被运行收据绑定".into());
    }
    let env = environment
        .get("APP_ENV")
        .map(String::as_str)
        .unwrap_or("dev");
    let mut app = load_app_table(&backend.join("config/app.toml"))?;
    let overrides = backend.join(format!("config/app.{env}.toml"));
    if overrides.is_file() {
        for (key, value) in load_app_table(&overrides)? {
            app.insert(key, value);
        }
    }
    let host = environment
        .get("APP_APP_HOST")
        .cloned()
        .or_else(|| app.get("host").cloned())
        .ok_or("应用配置缺少 host")?;
    let port = environment
        .get("APP_APP_PORT")
        .and_then(|value| value.parse::<u16>().ok())
        .or_else(|| app.get("port").and_then(|value| value.parse::<u16>().ok()))
        .ok_or("应用配置缺少有效 port")?;
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or("源 API 地址无效")?;
    let authority = rest.split('/').next().unwrap_or_default();
    let (url_host, url_port) = authority.rsplit_once(':').ok_or("源 API 地址缺少端口")?;
    let expected_hosts = match url_host {
        "127.0.0.1" => ["127.0.0.1", "0.0.0.0"],
        "[::1]" => ["::1", "::"],
        _ => return Err("源 API 地址必须为本机 loopback".into()),
    };
    if !expected_hosts.contains(&host.as_str())
        || url_port.parse::<u16>().ok() != Some(port)
        || rest.ends_with('/') && rest[rest.find('/').unwrap_or(rest.len())..].len() > 1
    {
        return Err("源 API 精确端口未绑定实际 APP 配置".into());
    }
    Ok(())
}

fn load_app_table(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("无法读取应用配置 {}：{error}", path.display()))?;
    let value: toml::Value = toml::from_str(&content)
        .map_err(|error| format!("无法解析应用配置 {}：{error}", path.display()))?;
    let app = value
        .get("app")
        .and_then(toml::Value::as_table)
        .ok_or("应用配置缺少 app")?;
    app.iter()
        .map(|(key, value)| {
            let value = match value {
                toml::Value::String(value) => value.clone(),
                toml::Value::Integer(value) => value.to_string(),
                _ => return Err(format!("应用配置 app.{key} 类型无效").into()),
            };
            Ok((key.clone(), value))
        })
        .collect()
}
