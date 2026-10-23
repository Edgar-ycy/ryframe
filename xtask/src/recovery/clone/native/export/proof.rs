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
    let (api_configuration, api_files) = runtime_configuration(context, &runtime, "api")?;
    let (worker_configuration, worker_files) = runtime_configuration(context, &runtime, "worker")?;
    if api_configuration["jobs"]["mode"].as_str() != Some("external")
        || worker_configuration["jobs"]["mode"].as_str() != Some("external")
    {
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
    verify_api_address(api, &api_configuration)?;
    verify_worker_address(worker, &worker_configuration)?;
    common::require_closed_port(api)?;
    common::require_closed_port(worker)?;
    if runtime_configuration(context, &runtime, "api")?.1 != api_files
        || runtime_configuration(context, &runtime, "worker")?.1 != worker_files
    {
        return Err("源运行配置在导出证明核验期间变化".into());
    }
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

fn runtime_configuration(
    context: &Context,
    runtime: &Value,
    role: &str,
) -> Result<(Value, BTreeMap<String, Value>)> {
    if context.environment != BTreeMap::from([("APP_ENV".into(), "test".into())]) {
        return Err("源导出只允许 APP_ENV=test；其他配置必须绑定 TOML 文件".into());
    }
    let declared = runtime
        .get(format!("{role}_config_dir"))
        .ok_or("源运行收据缺少角色配置目录")?;
    let (directory, files) =
        super::super::target_verify::input::request_configuration::configuration_files(
            &context.execution_backend,
            &context.environment,
            Some(declared),
        )?;
    let sha = super::super::target_verify::input::request_configuration::configuration_identity(
        &directory, &files,
    )?;
    if runtime[format!("{role}_configuration_sha256")].as_str() != Some(sha.as_str()) {
        return Err("源 TOML 配置与运行收据摘要不同；请核对绑定文件".into());
    }
    let table =
        crate::rate_limit::load_app_table(&context.execution_backend, Some(&directory), "test")
            .map_err(|_| "源 TOML 配置无法加载；请检查所选环境文件与字段类型")?;
    if super::super::target_verify::input::request_configuration::configuration_files(
        &context.execution_backend,
        &context.environment,
        Some(declared),
    )? != (directory, files.clone())
    {
        return Err("源 TOML 配置在读取期间变化".into());
    }
    Ok((serde_json::to_value(table)?, files))
}

fn verify_api_address(url: &str, configuration: &Value) -> Result<()> {
    let host = configuration["app"]["host"]
        .as_str()
        .ok_or("源 TOML 配置缺少 app.host")?;
    let port = configuration["app"]["port"]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or("源 TOML 配置缺少有效 app.port")?;
    let parsed = url::Url::parse(url).map_err(|_| "源 API 地址无效")?;
    let expected_hosts = match parsed.host_str() {
        Some("127.0.0.1") => ["127.0.0.1", "0.0.0.0"],
        Some("[::1]") => ["::1", "::"],
        _ => return Err("源 API 地址必须为本机 loopback".into()),
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || !expected_hosts.contains(&host)
        || parsed.port_or_known_default() != Some(port)
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("源 API 精确端口未绑定实际 TOML 配置".into());
    }
    Ok(())
}

fn verify_worker_address(url: &str, configuration: &Value) -> Result<()> {
    let jobs = &configuration["jobs"];
    let port = jobs["health_port"]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or("源 TOML 配置缺少有效 jobs.health_port")?;
    if jobs["health_host"].as_str() != Some("127.0.0.1")
        || url != format!("http://127.0.0.1:{port}/readyz")
    {
        return Err("源 Worker 就绪地址未绑定实际 TOML 配置".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_only_use_toml_fields_and_reject_ambiguous_urls() {
        let config = json!({"app":{"host":"0.0.0.0","port":18210},
            "jobs":{"health_host":"127.0.0.1","health_port":18211}});
        assert!(verify_api_address("http://127.0.0.1:18210", &config).is_ok());
        for url in [
            "http://127.0.0.1:18212",
            "http://127.0.0.1:18210/path",
            "http://127.0.0.1:18210?secret=x",
            "http://user@127.0.0.1:18210",
            "http://localhost:18210",
        ] {
            assert!(verify_api_address(url, &config).is_err());
        }
        assert!(verify_worker_address("http://127.0.0.1:18211/readyz", &config).is_ok());
        assert!(verify_worker_address("http://127.0.0.1:18212/readyz", &config).is_err());
        let ipv6 = json!({"app":{"host":"::","port":18210}});
        assert!(verify_api_address("http://[::1]:18210", &ipv6).is_ok());
    }
    #[test]
    fn runtime_digest_matches_selected_files_and_observes_all_toml_changes() {
        let temporary = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let root = temporary.path().to_path_buf();
        let directory = root.join("config");
        std::fs::create_dir(&directory).unwrap();
        let selected = directory.join("app.test.toml");
        std::fs::write(&selected, "[app]\nhost='127.0.0.1'\nport=18210\n").unwrap();
        let extra = directory.join("app.dev.toml");
        std::fs::write(&extra, "unused=true\n").unwrap();
        let canonical = std::fs::canonicalize(&directory).unwrap();
        let canonical = canonical.to_str().unwrap();
        let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
        let selected_sha = digest::file(&selected).unwrap().sha256;
        let payload = json!({"environment":{"APP_ENV":"test"},
            "config_directory":canonical,"files":{"app.test.toml":selected_sha}});
        let sha = super::super::super::target_verify::input::request_configuration::python_configuration_hash(&payload).unwrap();
        let runtime = json!({"api_config_dir":directory,"api_configuration_sha256":sha});
        let context = Context {
            backend: root.clone(),
            run_dir: root.clone(),
            mode: "run".into(),
            attempt: 1,
            manifest: Value::Null,
            request: Value::Null,
            request_path: root.join("request.json"),
            environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
            execution_backend: root,
        };
        let (table, before) = runtime_configuration(&context, &runtime, "api").unwrap();
        assert_eq!(table["app"]["port"], 18210);
        assert_eq!(before.len(), 2);
        std::fs::write(&extra, "unused=false\n").unwrap();
        let (_, after) = runtime_configuration(&context, &runtime, "api").unwrap();
        assert_ne!(before, after);
        std::fs::write(&selected, "[app]\nhost='127.0.0.1'\nport=18212\n").unwrap();
        assert!(runtime_configuration(&context, &runtime, "api").is_err());
        let mut overridden = context;
        overridden
            .environment
            .insert("APP_APP_PORT".into(), "18210".into());
        assert!(runtime_configuration(&overridden, &runtime, "api").is_err());
    }
}
