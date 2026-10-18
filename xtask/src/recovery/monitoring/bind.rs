use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::{
    Result,
    cli::MonitoringBindOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::monitoring::{
        binding::validate_binding,
        contract::{TOKEN_NAME, exact, read_document},
        repository,
        staging::{self, Credential},
        tools,
    },
};

pub(crate) trait Verifier {
    fn authority(&self, backend: &Path, runtime: &Path, target: &Path) -> Result<Value>;
    fn versions(&self, paths: &BTreeMap<String, PathBuf>) -> Result<BTreeMap<String, String>>;
    fn credential(&self, path: &Path) -> Result<Credential>;
}

struct LocalVerifier;

impl Verifier for LocalVerifier {
    fn credential(&self, path: &Path) -> Result<Credential> {
        staging::read_credential(path)
    }

    fn authority(&self, backend: &Path, runtime: &Path, target: &Path) -> Result<Value> {
        super::preflight::verify(backend, runtime, target)
    }

    fn versions(&self, paths: &BTreeMap<String, PathBuf>) -> Result<BTreeMap<String, String>> {
        tools::versions(paths)
    }
}

pub(crate) fn run(options: &MonitoringBindOptions, backend: &Path) -> Result<Value> {
    run_with(options, backend, &LocalVerifier)
}

pub(crate) fn run_with(
    options: &MonitoringBindOptions,
    backend: &Path,
    verifier: &impl Verifier,
) -> Result<Value> {
    let run = options.output.parent().ok_or("监控绑定输出缺少运行目录")?;
    let names = fs::read_dir(run)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    if names != BTreeSet::from([TOKEN_NAME.to_owned()]) {
        return Err("监控绑定要求只包含 metrics-token.txt 的新 run 目录".into());
    }
    let token_guard =
        super::execution::ProtectedInputs::open([options.metrics_token_file.clone()])?;
    let coordinator = repository::coordinator_binding(backend)?;
    let (rules, rule_documents) = repository::bind_rules(backend)?;
    let credential = verifier.credential(&options.metrics_token_file)?;
    let authority = verifier.authority(backend, &options.runtime_receipt, &options.target_plan)?;
    if load_secret(&authority, &credential)? != credential.secret {
        return Err("监控凭据与正式运行环境不同".into());
    }
    check_monitoring_ports(&authority, options)?;
    let tool_paths = tools::paths(options, backend)?;
    let source_guard = super::execution::ProtectedInputs::open(tool_paths.values().cloned())?;
    let versions = verifier.versions(&tool_paths)?;
    let staging_receipt = staging::create(
        backend,
        run,
        &coordinator,
        &credential,
        &tool_paths,
        &versions,
    )?;
    let staged = staging::verify_without_tools(run, &staging_receipt, &coordinator, &credential)?;
    let staged_guard = super::execution::ProtectedInputs::open(staged.files.values().cloned())?;
    let staged_tool_paths = versions
        .keys()
        .map(|name| {
            staged
                .files
                .get(&staging::tool_relative(name))
                .cloned()
                .map(|path| (name.clone(), path))
                .ok_or_else(|| "监控 staging 缺少工具".into())
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if verifier.versions(&staged_tool_paths)? != versions {
        return Err("监控 staging 工具版本与来源不同".into());
    }
    let tool_receipts = tools::receipts(&staged.files, &versions)?;
    let binding = binding_value(
        options,
        &authority,
        &coordinator,
        &staging_receipt,
        &credential,
        &tool_receipts,
        &rules,
    );
    validate_binding(&binding)?;
    verify_current_with(backend, &binding, verifier)?;
    for document in rule_documents {
        document.assert_unchanged()?;
    }
    token_guard.assert_unchanged()?;
    source_guard.assert_unchanged()?;
    staged_guard.assert_unchanged()?;
    crate::recovery::clone::native::json::write_new(&options.output, &binding)?;
    let mut result = binding;
    result["_binding"] = staging::descriptor(&options.output)?;
    Ok(result)
}

fn binding_value(
    options: &MonitoringBindOptions,
    authority: &Value,
    coordinator: &Value,
    staging_receipt: &Value,
    credential: &Credential,
    tool_receipts: &Value,
    rules: &Value,
) -> Value {
    json!({
        "format_version": 1,
        "kind": "restore-monitoring-binding",
        "status": "bound",
        "run_id": options.run_id,
        "scope_id": authority["restore"]["scope_id"],
        "authority": authority,
        "coordinator": coordinator,
        "staging": staging_receipt,
        "environment": authority["environment"],
        "credential": credential.absolute,
        "tools": tool_receipts,
        "rules": rules,
        "endpoints": {
            "prometheus": format!("http://127.0.0.1:{}", options.ports.prometheus),
            "alertmanager": format!("http://127.0.0.1:{}", options.ports.alertmanager),
            "webhook": format!("http://127.0.0.1:{}", options.ports.webhook),
        },
        "ports": {
            "prometheus": options.ports.prometheus,
            "alertmanager": options.ports.alertmanager,
            "webhook": options.ports.webhook,
        },
        "contact_policy": {
            "loopback_only": true,
            "send_resolved": true,
            "external_receivers": [],
        },
        "allowed_writes": ["alertmanager-data", "configs", "evidence", "processes", "prometheus-data"],
        "created_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    })
}

pub(crate) fn verify_current(backend: &Path, binding: &Value) -> Result<()> {
    verify_current_with(backend, binding, &LocalVerifier)
}

pub(crate) fn verify_current_with(
    backend: &Path,
    binding: &Value,
    verifier: &impl Verifier,
) -> Result<()> {
    validate_binding(binding)?;
    if repository::coordinator_binding(backend)? != binding["coordinator"] {
        return Err("监控协调源码在绑定后发生变化".into());
    }
    let (rules, rule_documents) = repository::bind_rules(backend)?;
    if rules != binding["rules"] {
        return Err("监控规则在绑定后发生变化".into());
    }
    let runtime_path = PathBuf::from(
        binding
            .pointer("/authority/runtime/path")
            .and_then(Value::as_str)
            .ok_or("监控绑定缺少运行收据路径")?,
    );
    let target_path = PathBuf::from(
        binding
            .pointer("/authority/target_plan/path")
            .and_then(Value::as_str)
            .ok_or("监控绑定缺少目标计划路径")?,
    );
    if verifier.authority(backend, &runtime_path, &target_path)? != binding["authority"] {
        return Err("监控正式运行权威在绑定后发生变化".into());
    }
    let credential = verifier.credential(Path::new(
        binding["credential"]["path"]
            .as_str()
            .ok_or("监控绑定缺少凭据路径")?,
    ))?;
    if credential.absolute != binding["credential"]
        || load_secret(&binding["authority"], &credential)? != credential.secret
    {
        return Err("监控凭据文件在绑定后发生变化".into());
    }
    let run = Path::new(
        binding["staging"]["path"]
            .as_str()
            .ok_or("监控绑定缺少 staging 清单")?,
    )
    .parent()
    .and_then(Path::parent)
    .ok_or("监控 staging 路径无效")?;
    let staged = staging::verify(
        run,
        &binding["staging"],
        &binding["coordinator"],
        &credential,
        binding["tools"].as_object().ok_or("监控绑定缺少工具")?,
    )?;
    let versions = staged
        .manifest
        .get("tool_versions")
        .and_then(Value::as_object)
        .ok_or("监控 staging 缺少工具版本")?
        .iter()
        .map(|(name, value)| {
            value
                .as_str()
                .map(|value| (name.clone(), value.to_owned()))
                .ok_or_else(|| "监控 staging 工具版本无效".into())
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if tools::receipts(&staged.files, &versions)? != binding["tools"] {
        return Err("监控 staging 执行输入在绑定后发生变化".into());
    }
    for document in rule_documents {
        document.assert_unchanged()?;
    }
    Ok(())
}

pub(crate) fn load_environment(
    backend: &Path,
    binding: &Value,
) -> Result<(String, crate::recovery::monitoring::contract::Document)> {
    let descriptor = binding.get("environment").ok_or("监控绑定缺少环境")?;
    let path = PathBuf::from(
        descriptor
            .get("path")
            .and_then(Value::as_str)
            .ok_or("监控运行环境路径无效")?,
    );
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
    let document = read_document(&path, "监控运行环境")?;
    if &document.descriptor() != descriptor {
        return Err("监控运行环境与正式启动绑定不同".into());
    }
    let object = exact(&document.value, &["environment"], "监控私有环境")?;
    let environment = object
        .get("environment")
        .and_then(Value::as_object)
        .ok_or("监控私有环境必须是固定文本映射")?;
    if environment.values().any(|value| !value.is_string()) {
        return Err("监控私有环境值必须是字符串".into());
    }
    let credential = staging::read_credential(Path::new(
        binding["credential"]["path"]
            .as_str()
            .ok_or("监控凭据路径无效")?,
    ))?;
    if credential.absolute != binding["credential"]
        || environment
            .get("APP_MONITOR_METRICS_BEARER_TOKEN")
            .and_then(Value::as_str)
            != Some(credential.secret.as_str())
        || environment.get("APP_SCOPE_ID").and_then(Value::as_str) != binding["scope_id"].as_str()
    {
        return Err("监控指标凭据与正式运行环境不同".into());
    }
    document.assert_unchanged()?;
    Ok((credential.secret, document))
}

fn load_secret(authority: &Value, credential: &Credential) -> Result<String> {
    let path = PathBuf::from(
        authority
            .pointer("/environment/path")
            .and_then(Value::as_str)
            .ok_or("监控权威缺少环境路径")?,
    );
    let document = read_document(&path, "监控运行环境")?;
    let secret = document
        .value
        .pointer("/environment/APP_MONITOR_METRICS_BEARER_TOKEN")
        .and_then(Value::as_str)
        .ok_or("监控运行环境缺少指标凭据")?
        .to_owned();
    if secret != credential.secret {
        return Err("监控指标凭据与正式运行环境不同".into());
    }
    document.assert_unchanged()?;
    Ok(secret)
}

fn check_monitoring_ports(authority: &Value, options: &MonitoringBindOptions) -> Result<()> {
    let product = authority
        .get("endpoints")
        .and_then(Value::as_object)
        .ok_or("监控权威缺少产品端点")?;
    let product_ports = product
        .values()
        .filter_map(Value::as_str)
        .filter_map(|value| {
            crate::recovery::monitoring::http::loopback_url(value)
                .ok()
                .map(|(address, _, _)| address.port())
        })
        .collect::<BTreeSet<_>>();
    let ports = [
        options.ports.prometheus,
        options.ports.alertmanager,
        options.ports.webhook,
    ];
    if ports.iter().any(|port| product_ports.contains(port)) {
        return Err("监控端口不能占用产品端点端口".into());
    }
    for port in ports {
        crate::recovery::clone::native::resources::common::require_closed_port(&format!(
            "http://127.0.0.1:{port}"
        ))?;
    }
    Ok(())
}
