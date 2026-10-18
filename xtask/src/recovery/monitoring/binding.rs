use std::{collections::BTreeSet, path::Path};

use serde_json::{Map, Value};

use super::contract::{
    STAGING_DIRECTORY, STAGING_MANIFEST, TOKEN_NAME, absolute_text, artifact, exact, hex_text,
    loopback_port, process_identity, receipt_descriptor, timestamp, valid_run_id,
};
use crate::Result;

pub(crate) fn validate_binding(value: &Value) -> Result<Value> {
    let binding = exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "run_id",
            "scope_id",
            "authority",
            "coordinator",
            "staging",
            "environment",
            "credential",
            "tools",
            "rules",
            "endpoints",
            "ports",
            "contact_policy",
            "allowed_writes",
            "created_at",
        ],
        "监控投递绑定",
    )?;
    if binding.get("format_version").and_then(Value::as_u64) != Some(1)
        || binding.get("kind").and_then(Value::as_str) != Some("restore-monitoring-binding")
        || binding.get("status").and_then(Value::as_str) != Some("bound")
    {
        return Err("监控投递绑定版本、类型或状态无效".into());
    }
    let authority = validate_authority(binding.get("authority").ok_or("监控绑定缺少权威")?)?;
    let run_id = binding
        .get("run_id")
        .and_then(Value::as_str)
        .filter(|value| valid_run_id(value))
        .ok_or("监控 run-id 无效")?;
    let scope = authority["restore"]["scope_id"]
        .as_str()
        .ok_or("监控权威 scope 无效")?;
    if binding.get("scope_id").and_then(Value::as_str) != Some(scope) {
        return Err("监控投递 run 或 scope 无效".into());
    }
    let coordinator = exact(
        binding.get("coordinator").ok_or("监控绑定缺少协调来源")?,
        &["root", "head", "inventory_sha256", "executable"],
        "监控协调源码",
    )?;
    let coordinator_root = absolute_text(coordinator.get("root"), "监控协调源码根")?;
    hex_text(coordinator.get("head"), 40, "监控协调提交")?;
    hex_text(coordinator.get("inventory_sha256"), 64, "监控协调库存摘要")?;
    receipt_descriptor(
        coordinator
            .get("executable")
            .ok_or("监控协调来源缺少可执行文件")?,
        "监控协调可执行文件",
        true,
    )?;
    let staging = receipt_descriptor(
        binding.get("staging").ok_or("监控绑定缺少 staging")?,
        "监控 staging 清单",
        true,
    )?;
    let staging_path = absolute_text(staging.get("path"), "监控 staging 清单")?;
    if staging_path.file_name().and_then(|value| value.to_str()) != Some(STAGING_MANIFEST)
        || staging_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            != Some(STAGING_DIRECTORY)
    {
        return Err("监控 staging 清单路径无效".into());
    }
    let staging_root = staging_path.parent().ok_or("监控 staging 缺少目录")?;
    let run_directory = staging_root.parent().ok_or("监控 staging 缺少 run 目录")?;
    if !run_directory.starts_with(coordinator_root.join(".local-tests")) {
        return Err("监控 staging 必须位于协调器忽略目录".into());
    }
    if binding.get("environment") != authority.get("environment") {
        return Err("监控绑定环境与正式运行权威不同".into());
    }
    receipt_descriptor(
        binding.get("environment").ok_or("监控绑定缺少环境")?,
        "监控运行环境",
        true,
    )?;
    let credential = exact(
        binding.get("credential").ok_or("监控绑定缺少凭据")?,
        &[
            "path",
            "bytes",
            "sha256",
            "device",
            "inode",
            "security_sha256",
        ],
        "监控凭据",
    )?;
    if absolute_text(credential.get("path"), "监控凭据")? != run_directory.join(TOKEN_NAME)
        || credential
            .get("bytes")
            .and_then(Value::as_u64)
            .is_none_or(|value| value == 0)
        || credential.get("device").and_then(Value::as_u64).is_none()
        || credential.get("inode").and_then(Value::as_u64).is_none()
    {
        return Err("监控凭据绑定无效".into());
    }
    hex_text(credential.get("sha256"), 64, "监控凭据摘要")?;
    hex_text(credential.get("security_sha256"), 64, "监控凭据权限摘要")?;
    validate_staging_inputs(binding, staging_root, &coordinator_root)?;
    validate_endpoints(binding)?;
    timestamp(binding.get("created_at"), "监控绑定时间")?;
    let _ = run_id;
    Ok(value.clone())
}

fn validate_staging_inputs(
    binding: &Map<String, Value>,
    staging_root: &Path,
    coordinator_root: &Path,
) -> Result<()> {
    let tools = exact(
        binding.get("tools").ok_or("监控绑定缺少工具")?,
        &["prometheus", "promtool", "alertmanager", "amtool"],
        "监控工具集",
    )?;
    for name in ["prometheus", "promtool", "alertmanager", "amtool"] {
        let tool = tool_receipt(tools.get(name).ok_or("监控工具集缺少登记项")?, name)?;
        let expected = staging_root
            .join("tools")
            .join(format!("{name}{}", if cfg!(windows) { ".exe" } else { "" }));
        if absolute_text(tool.get("path"), "监控工具")? != expected
            || tool
                .get("version")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(format!("监控工具 {name} 必须来自固定 staging 路径").into());
        }
    }
    let rules = exact(
        binding.get("rules").ok_or("监控绑定缺少规则")?,
        &["alerts", "tests"],
        "监控规则",
    )?;
    for name in ["alerts", "tests"] {
        let rule = rules.get(name).ok_or("监控规则缺少登记项")?;
        receipt_descriptor(rule, "监控规则", true)?;
        artifact(rule, "监控规则", None)?;
    }
    for (name, filename) in [
        ("alerts", "ryframe-alerts.yml"),
        ("tests", "ryframe-alerts.test.yml"),
    ] {
        let path = absolute_text(
            rules.get(name).and_then(|value| value.get("path")),
            "监控规则",
        )?;
        if path
            != coordinator_root
                .join("deploy")
                .join("prometheus")
                .join(filename)
        {
            return Err("监控规则路径不是协调器内固定的正式文件".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_authority(value: &Value) -> Result<Value> {
    let authority = exact(
        value,
        &[
            "format_version",
            "kind",
            "runtime",
            "target_plan",
            "launch",
            "environment",
            "restore",
            "source",
            "endpoints",
            "maintenance_execution",
            "product_execution",
            "processes",
            "source_generation",
            "dataset_lineage",
        ],
        "监控运行权威",
    )?;
    if authority.get("format_version").and_then(Value::as_u64) != Some(1)
        || authority.get("kind").and_then(Value::as_str) != Some("restore-monitoring-authority")
    {
        return Err("监控运行权威版本或类型无效".into());
    }
    for name in [
        "runtime",
        "target_plan",
        "launch",
        "environment",
        "source_generation",
        "dataset_lineage",
    ] {
        receipt_descriptor(
            authority.get(name).ok_or("监控运行权威缺少证据")?,
            "监控权威",
            true,
        )?;
    }
    let restore = exact(
        authority.get("restore").ok_or("监控运行权威缺少恢复身份")?,
        &[
            "id",
            "backup_id",
            "plan_hash",
            "scope_id",
            "data_verified_at",
        ],
        "监控恢复身份",
    )?;
    for name in ["id", "backup_id", "scope_id"] {
        if restore
            .get(name)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err("监控恢复身份无效".into());
        }
    }
    hex_text(restore.get("plan_hash"), 64, "监控恢复计划摘要")?;
    timestamp(restore.get("data_verified_at"), "监控数据核验时间")?;
    validate_authority_origin(authority)?;
    validate_product_execution(authority)?;
    Ok(value.clone())
}

fn validate_authority_origin(authority: &Map<String, Value>) -> Result<()> {
    let source = exact(
        authority.get("source").ok_or("监控运行权威缺少来源")?,
        &[
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
        ],
        "监控恢复来源",
    )?;
    for name in [
        "backup_source_sha",
        "backend_product_sha",
        "backend_execution_sha",
        "frontend_sha",
    ] {
        hex_text(source.get(name), 40, "监控恢复来源提交")?;
    }
    let same = source.get("backend_product_sha") == source.get("backend_execution_sha");
    let adapter = source.get("backend_adapter_contract");
    if (same && !adapter.is_some_and(Value::is_null))
        || (!same && adapter.and_then(Value::as_str) != Some("legacy-stable-readiness-b0-v1"))
    {
        return Err("监控恢复来源与适配合同不一致".into());
    }
    let endpoints = exact(
        authority.get("endpoints").ok_or("监控运行权威缺少端点")?,
        &[
            "api_ready",
            "worker_ready",
            "frontend",
            "api_metrics",
            "worker_metrics",
        ],
        "监控产品端点",
    )?;
    for (name, path) in [
        ("api_ready", "/readyz"),
        ("worker_ready", "/readyz"),
        ("frontend", ""),
        ("api_metrics", "/api/v1/monitor/metrics"),
        ("worker_metrics", "/metrics"),
    ] {
        loopback_port(
            endpoints
                .get(name)
                .and_then(Value::as_str)
                .ok_or("监控产品端点无效")?,
            path,
        )?;
    }
    let maintenance = exact(
        authority
            .get("maintenance_execution")
            .ok_or("缺少维护来源")?,
        &["root", "binding", "build"],
        "监控维护来源",
    )?;
    absolute_text(maintenance.get("root"), "监控维护源码根")?;
    receipt_descriptor(
        maintenance.get("build").ok_or("缺少维护构建")?,
        "监控维护构建",
        true,
    )?;
    Ok(())
}

fn validate_product_execution(authority: &Map<String, Value>) -> Result<()> {
    let product = exact(
        authority.get("product_execution").ok_or("缺少产品来源")?,
        &[
            "roots",
            "backend_product_sha",
            "backend_execution_sha",
            "frontend_sha",
            "builds",
            "adapter",
        ],
        "监控产品执行来源",
    )?;
    let roots = exact(
        product.get("roots").ok_or("缺少产品源码根")?,
        &["source_backend", "execution_backend", "frontend"],
        "监控产品源码根",
    )?;
    for name in ["source_backend", "execution_backend", "frontend"] {
        absolute_text(roots.get(name), "监控产品源码根")?;
    }
    for name in [
        "backend_product_sha",
        "backend_execution_sha",
        "frontend_sha",
    ] {
        hex_text(product.get(name), 40, "监控产品来源提交")?;
    }
    let builds = exact(
        product.get("builds").ok_or("缺少产品构建")?,
        &["backend", "frontend"],
        "监控产品构建",
    )?;
    for name in ["backend", "frontend"] {
        receipt_descriptor(
            builds.get(name).ok_or("缺少产品构建收据")?,
            "监控产品构建",
            true,
        )?;
    }
    let processes = exact(
        authority.get("processes").ok_or("监控运行权威缺少进程")?,
        &["api", "worker", "frontend"],
        "监控产品进程",
    )?;
    for name in ["api", "worker", "frontend"] {
        process_identity(processes.get(name).ok_or("监控产品进程缺少登记项")?, name)?;
    }
    Ok(())
}

fn validate_endpoints(binding: &Map<String, Value>) -> Result<()> {
    let endpoints = exact(
        binding.get("endpoints").ok_or("监控绑定缺少端点")?,
        &["prometheus", "alertmanager", "webhook"],
        "监控投递端点",
    )?;
    let ports = exact(
        binding.get("ports").ok_or("监控绑定缺少端口")?,
        &["prometheus", "alertmanager", "webhook"],
        "监控投递端口",
    )?;
    let mut unique = BTreeSet::new();
    for name in ["prometheus", "alertmanager", "webhook"] {
        let port = loopback_port(
            endpoints
                .get(name)
                .and_then(Value::as_str)
                .ok_or("监控投递端点无效")?,
            "",
        )?;
        if ports.get(name).and_then(Value::as_u64) != Some(u64::from(port))
            || !unique.insert(port)
            || !(1024..=65535).contains(&port)
        {
            return Err("监控投递只能使用绑定的互异 loopback 端口".into());
        }
    }
    let policy = exact(
        binding
            .get("contact_policy")
            .ok_or("监控绑定缺少联系人策略")?,
        &["loopback_only", "send_resolved", "external_receivers"],
        "监控联系人策略",
    )?;
    if policy.get("loopback_only").and_then(Value::as_bool) != Some(true)
        || policy.get("send_resolved").and_then(Value::as_bool) != Some(true)
        || policy
            .get("external_receivers")
            .and_then(Value::as_array)
            .is_none_or(|value| !value.is_empty())
    {
        return Err("监控投递不得包含真实业务联系人".into());
    }
    let writes = binding.get("allowed_writes").and_then(Value::as_array);
    if writes.is_none_or(|value| {
        value
            != &[
                "alertmanager-data",
                "configs",
                "evidence",
                "processes",
                "prometheus-data",
            ]
            .map(Value::from)
    }) {
        return Err("监控投递写入边界无效".into());
    }
    Ok(())
}

fn staged_receipt(value: &Value, label: &str) -> Result<Value> {
    let receipt = exact(
        value,
        &["path", "bytes", "sha256", "device", "inode"],
        label,
    )?;
    if receipt.get("device").and_then(Value::as_u64).is_none()
        || receipt.get("inode").and_then(Value::as_u64).is_none()
    {
        return Err(format!("{label}文件身份无效").into());
    }
    receipt_descriptor(
        &serde_json::json!({
            "path": receipt.get("path"),
            "bytes": receipt.get("bytes"),
            "sha256": receipt.get("sha256"),
        }),
        label,
        false,
    )?;
    Ok(value.clone())
}

fn tool_receipt(value: &Value, name: &str) -> Result<Value> {
    let receipt = exact(
        value,
        &["path", "bytes", "sha256", "device", "inode", "version"],
        name,
    )?;
    staged_receipt(
        &serde_json::json!({
            "path": receipt.get("path"),
            "bytes": receipt.get("bytes"),
            "sha256": receipt.get("sha256"),
            "device": receipt.get("device"),
            "inode": receipt.get("inode"),
        }),
        name,
    )?;
    Ok(value.clone())
}
