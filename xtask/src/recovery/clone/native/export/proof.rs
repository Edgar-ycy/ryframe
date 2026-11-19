use std::{collections::BTreeMap, path::Path};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{
    bridge, digest,
    json::{self as json_io, array, exact, object, text},
    lifecycle::manifest,
    resources::common,
};
use super::input::Context;
use crate::Result;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/export/producers.rs"]
mod producer_tests;

pub(crate) mod copy;

pub(crate) fn generation(context: &Context) -> Result<Value> {
    let bindings = array(&context.manifest["build_bridges"], "显式构建来源审计")?;
    let scope = bridge::scope::Scope::load(&context.backend, bindings, &[])?;
    generation_in(context, &scope)
}

pub(crate) fn generation_in(context: &Context, scope: &bridge::scope::Scope) -> Result<Value> {
    generation_observed(context, scope, common::require_closed_runtime_ports, false)
}

fn generation_observed(
    context: &Context,
    scope: &bridge::scope::Scope,
    require_ports: fn(&str, &str) -> Result<()>,
    require_producer_names: bool,
) -> Result<Value> {
    let build_path = manifest::bound_file(
        &context.backend,
        context.request.get("backend_build").unwrap(),
        "API/Worker 构建",
        true,
    )?;
    let build = json_io::read(&build_path)?;
    let fingerprint = text(&context.request["worktree_fingerprint"], "源请求构建指纹")?;
    let expected = scope.product(&context.execution_backend, &build_path, &build, fingerprint)?;
    let maintenance = verify_maintenance(context, &expected, scope)?;
    let runtime_path = manifest::bound_file(
        &context.backend,
        context.request.get("runtime").unwrap(),
        "源运行收据",
        true,
    )?;
    let runtime = json_io::read(&runtime_path)?;
    verify_runtime_contract(context, &runtime_path, &runtime)?;
    if runtime.get("scope_id").and_then(Value::as_str)
        != context.source().get("scope_id").and_then(Value::as_str)
    {
        return Err("源 runtime scope 与请求不一致".into());
    }
    let processes = runtime_processes(context, &build, &runtime)?;
    let producers = producers(context, &processes, require_producer_names)?;
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
    require_ports(api, worker)?;
    if runtime_configuration(context, &runtime, "api")?.1 != api_files
        || runtime_configuration(context, &runtime, "worker")?.1 != worker_files
    {
        return Err("源运行配置在导出证明核验期间变化".into());
    }
    verify_runtime_contract(context, &runtime_path, &runtime)?;
    let proof = json!({
        "source": expected["snapshot"],
        "worktree_fingerprint": context.request["worktree_fingerprint"],
        "runtime": runtime,
        "processes": processes,
        "operator_declared_producers": producers,
        "physical_binding": physical_binding(context, &runtime)?,
        "maintenance": maintenance,
        "request_sha256": digest::plan_hash(&context.request)?,
        "external_writers_discovered": false,
        "clone_verified": false,
        "restore_qualified": false,
    });
    scope.assert_unchanged()?;
    Ok(proof)
}

fn verify_runtime_contract(context: &Context, path: &Path, expected: &Value) -> Result<()> {
    super::super::seed::runtime_model::validate_api_environment(
        &context.backend,
        &context.environment,
    )?;
    let directory = path.parent().ok_or("源运行收据缺少所在目录")?;
    let actual = crate::recovery::fixture_control::verify_full_stack_runtime(
        &context.execution_backend,
        directory,
    )?;
    if actual != *expected {
        return Err("源完整运行契约与已读取的运行收据不同".into());
    }
    let bound = manifest::bound_file(
        &context.backend,
        &context.request["runtime"],
        "源运行收据",
        true,
    )?;
    if bound != path {
        return Err("源运行收据不属于实际绑定路径".into());
    }
    Ok(())
}

fn runtime_processes(
    context: &Context,
    build: &Value,
    runtime: &Value,
) -> Result<BTreeMap<String, Value>> {
    let artifacts = object(&build["artifacts"], "API/Worker 构建产物")?;
    let runtime_artifacts = object(&runtime["artifacts"], "源 runtime 产物")?;
    let mut processes = BTreeMap::new();
    for role in ["api", "worker"] {
        let artifact = exact(
            artifacts.get(role).ok_or("API/Worker 构建缺少产物")?,
            &["executable", "command", "bytes", "sha256"],
            "API/Worker 构建产物",
        )?;
        let executable = text(&artifact["executable"], "产物路径")?;
        let expected = json!({"path":executable,"sha256":text(&artifact["sha256"],"产物摘要")?});
        if runtime_artifacts.get(role) != Some(&expected) {
            return Err("源 runtime 产物与当前构建不一致".into());
        }
        let descriptor = context
            .request
            .pointer(&format!("/processes/{role}"))
            .ok_or("源请求缺少进程收据")?;
        let path = manifest::bound_file(&context.backend, descriptor, "源进程收据", true)?;
        let value = json_io::read(&path)?;
        let receipt = exact(
            &value,
            &["format_version", "role", "scope_id", "identity"],
            "源进程收据",
        )?;
        validate_identity(&receipt["identity"], "源进程身份")?;
        if receipt["format_version"].as_u64() != Some(1)
            || receipt["role"] != role
            || receipt.get("scope_id") != context.source().get("scope_id")
            || value
                .pointer("/identity/executable")
                .and_then(Value::as_str)
                != Some(executable)
        {
            return Err("源进程角色、scope 或实际二进制与构建不一致".into());
        }
        manifest::bound_file(&context.backend, descriptor, "源进程收据", true)?;
        processes.insert(role.to_owned(), receipt["identity"].clone());
    }
    Ok(processes)
}

fn verify_maintenance(
    context: &Context,
    expected: &Value,
    scope: &bridge::scope::Scope,
) -> Result<Value> {
    let path = manifest::bound_file(
        &context.execution_backend,
        context.request.get("maintenance_build").unwrap(),
        "维护构建",
        true,
    )?;
    let value = json_io::read(&path)?;
    let receipt = scope.maintenance(&context.execution_backend, &path, &value)?;
    if receipt.pointer("/source/snapshot") != expected.get("snapshot")
        || receipt.pointer("/source/worktree_fingerprint")
            != context.request.get("worktree_fingerprint")
    {
        return Err("维护工具与源实际后端构建不属于同一完整源码".into());
    }
    Ok(receipt)
}

fn producers(
    context: &Context,
    processes: &BTreeMap<String, Value>,
    require_names: bool,
) -> Result<Vec<Value>> {
    let path = manifest::bound_file(
        &context.backend,
        context.request.get("producers_registry").unwrap(),
        "producer 清单",
        true,
    )?;
    let registry = json_io::read(&path)?;
    let runtime_sha = text(&context.request["runtime"]["sha256"], "源运行收据摘要")?;
    let result = validate_registry(
        &registry,
        &context.source()["scope_id"],
        runtime_sha,
        processes,
    )?;
    if require_names {
        for producer in &result {
            super::input::validate_name(text(&producer["name"], "producer 名称")?)?;
        }
    }
    for producer in &result {
        super::super::seed::source_registration::quiet::require_stopped(
            &context.backend,
            &context.run_dir,
            &producer["identity"],
            &crate::recovery::fixture_control::portable_path(&std::env::current_exe()?),
        )?;
    }
    manifest::bound_file(
        &context.backend,
        &context.request["producers_registry"],
        "producer 清单",
        true,
    )?;
    Ok(result)
}

pub(crate) fn validate_registry(
    registry: &Value,
    scope: &Value,
    runtime_sha: &str,
    processes: &BTreeMap<String, Value>,
) -> Result<Vec<Value>> {
    text(scope, "源运行 scope")?;
    let registry = exact(
        registry,
        &["format_version", "scope_id", "runtime_sha256", "processes"],
        "producer 清单",
    )?;
    let items = array(registry.get("processes").unwrap(), "producer 清单")?;
    if registry.get("format_version").and_then(Value::as_u64) != Some(1)
        || registry.get("scope_id") != Some(scope)
        || registry.get("runtime_sha256").and_then(Value::as_str) != Some(runtime_sha)
        || items.len() < 3
    {
        return Err("操作人声明的 producer 清单不完整或 scope 不同".into());
    }
    let mut names = std::collections::BTreeSet::new();
    let mut identities = std::collections::BTreeSet::new();
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let item = exact(item, &["name", "identity"], "producer")?;
        let name = text(item.get("name").unwrap(), "producer 名称")?;
        let identity = validate_identity(item.get("identity").unwrap(), "producer 身份")?;
        if !names.insert(name.to_owned()) {
            return Err("producer 名称重复".into());
        }
        let key = (
            identity["pid"].as_u64().ok_or("producer PID 无效")?,
            text(&identity["started"], "producer 创建时间")?.to_owned(),
        );
        if !identities.insert(key) {
            return Err("producer 创建代次重复，不能用不同名称重复登记同一进程".into());
        }
        if processes
            .get(name)
            .is_some_and(|recorded| recorded != identity)
        {
            return Err("producer 清单中的 API/Worker 代次与真实运行收据不同".into());
        }
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

fn physical_binding(context: &Context, runtime: &Value) -> Result<Value> {
    let configuration = super::super::s3::app_configuration::configuration_value(
        &context.backend,
        Path::new(text(&runtime["api_config_dir"], "源API配置目录")?),
        "test",
    )?;
    if context.source()["s3"]["configuration"] != configuration {
        return Err("源对象绑定不属于实际API运行配置；请重新登记来源".into());
    }
    crate::recovery::source::evidence::binding::physical(&json!({
        "backend": context.execution_backend,
        "evidence_root": context.backend,
        "source": context.source(),
        "app_env": "test",
        "config_dir": runtime["api_config_dir"],
    }))
}

fn runtime_configuration(
    context: &Context,
    runtime: &Value,
    role: &str,
) -> Result<(Value, BTreeMap<String, Value>)> {
    super::super::s3::app_configuration::runtime_configuration(
        &context.backend,
        &context.execution_backend,
        &context.environment,
        runtime,
        role,
    )
}

pub(crate) fn verify_api_address(url: &str, configuration: &Value) -> Result<()> {
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

pub(crate) fn verify_worker_address(url: &str, configuration: &Value) -> Result<()> {
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
            output_dir: root.join("e0001"),
            mode: "run".into(),
            attempt: 1,
            manifest: Value::Null,
            object_identity: json!({}),
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

    #[test]
    fn external_product_uses_coordinator_role_configuration_without_rebinding() {
        use super::super::super::target_verify::input::request_configuration::{
            configuration_files, configuration_identity,
        };

        let fixture = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
        let coordinator = fixture.path().to_path_buf();
        let local = coordinator.join(".local-tests");
        let execution = local.join("published/backend");
        let api = local.join("run/post-copy/runtime-configuration/config");
        let worker = local.join("run/post-copy/runtime-configuration/worker-config");
        let private = local.join("private-temp");
        let environment = BTreeMap::from([
            ("APP_ENV".to_owned(), "test".to_owned()),
            ("TEMP".to_owned(), private.to_str().unwrap().to_owned()),
            ("TMP".to_owned(), private.to_str().unwrap().to_owned()),
        ]);
        let selected = "[app]\nhost='127.0.0.1'\nport=18210\n";
        for directory in [&execution, &api, &worker, &private] {
            std::fs::create_dir_all(directory).unwrap();
        }
        for directory in [&api, &worker] {
            std::fs::write(directory.join("app.test.toml"), selected).unwrap();
        }
        let outside = coordinator.join("unmanaged-config");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("app.test.toml"), selected).unwrap();
        let contract_sha = |directory: &Path| {
            let (directory, files) =
                configuration_files(&coordinator, &environment, Some(&json!(directory))).unwrap();
            configuration_identity(&directory, &files).unwrap()
        };
        let runtime = json!({"backend_root":execution,
            "api_config_dir":api,"api_configuration_sha256":contract_sha(&api),
            "worker_config_dir":worker,"worker_configuration_sha256":contract_sha(&worker)});
        let outside_sha = contract_sha(&outside);
        let context = Context {
            backend: coordinator.clone(),
            run_dir: local.join("run"),
            output_dir: local.join("run/e0001"),
            mode: "source-register".into(),
            attempt: 1,
            manifest: Value::Null,
            object_identity: json!({}),
            request: Value::Null,
            request_path: local.join("run/request.json"),
            environment,
            execution_backend: execution,
        };

        for role in ["api", "worker"] {
            let (table, files) = runtime_configuration(&context, &runtime, role).unwrap();
            assert_eq!(table["app"]["port"], 18210);
            assert_eq!(files.len(), 1);
        }
        std::fs::write(worker.join("app.test.toml"), "[app]\nport=18211\n").unwrap();
        assert!(runtime_configuration(&context, &runtime, "worker").is_err());

        let mut rebound = runtime;
        rebound["api_config_dir"] = json!(outside);
        rebound["api_configuration_sha256"] = json!(outside_sha);
        assert!(runtime_configuration(&context, &rebound, "api").is_err());
    }
}
