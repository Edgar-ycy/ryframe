//! 采集并核验目标计划的私有环境及实际 TOML 启动快照。
use super::{check, configuration_registration, inputs::Inputs, path};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as strict, target_verify::input::request_configuration},
        inputs::document::{self, Document},
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[path = "environment/legacy.rs"]
mod legacy;
#[path = "environment/legacy_runtime.rs"]
mod legacy_runtime;
pub(in crate::recovery::runtime) use legacy_runtime::{configure, materialize, verify_projected};

pub(in crate::recovery::runtime) fn capture(inputs: &mut Inputs, request: &Value) -> Result<Value> {
    let registration = inputs.bound(&request["registration"]["registration"])?;
    let document = configuration_registration::read(inputs, &registration)?;
    configuration(request, &document)
}

pub(in crate::recovery::runtime) fn verify(inputs: &mut Inputs, request: &Value) -> Result<()> {
    check(
        capture(inputs, request)? == request["environment"],
        "恢复启动角色配置快照发生漂移",
    )
}

fn configuration(request: &Value, document: &Document) -> Result<Value> {
    let private = &document.value;
    strict::exact(
        private,
        &[
            "metrics_bearer_token",
            "scope_id",
            "api_config_dir",
            "worker_config_dir",
        ],
        "恢复私有环境",
    )?;
    for value in strict::object(private, "恢复私有环境")?.values() {
        let text = strict::text(value, "恢复私有环境字段")?;
        check(
            !text.is_empty() && text.trim() == text && !text.chars().any(char::is_control),
            "恢复私有环境必须使用非空明文字符串",
        )?;
    }
    check(
        private["scope_id"] == request["authority"]["scope_id"],
        "恢复私有环境 scope 不同",
    )?;
    let execution = path(&request["roots"]["backend_execution"])?;
    let frontend = crate::recovery::inputs::domain::endpoint(
        &request["authority"]["frontend_endpoint"],
        "恢复前端端点",
        Some(""),
    )?;
    check(
        frontend.scheme == "http",
        "恢复运行私有配置要求本机 HTTP 前端端点",
    )?;
    let environment = BTreeMap::from([("APP_ENV".to_owned(), "test".to_owned())]);
    let evidence = document.path.parent().ok_or("恢复环境缺少证据目录")?;
    let mut identifiers = std::collections::BTreeSet::new();
    let mut configurations = serde_json::Map::new();
    for role_name in ["api", "worker"] {
        let (id, snapshot) = role(
            request,
            private,
            &execution,
            evidence,
            &environment,
            role_name,
        )?;
        check(
            identifiers.insert(id),
            "恢复 API 与 Worker 实例编号必须不同",
        )?;
        configurations.insert(role_name.to_owned(), snapshot);
    }
    document.assert_unchanged()?;
    Ok(json!({"document":document::binding(document),"configurations":configurations}))
}

fn role(
    request: &Value,
    private: &Value,
    execution: &Path,
    evidence: &Path,
    environment: &BTreeMap<String, String>,
    role: &str,
) -> Result<(i64, Value)> {
    let declared = &private[format!("{role}_config_dir")];
    let directory = path(declared)?;
    // 配置可位于执行根内，或协调器本次私有环境文件的证据目录内。
    let base = if directory.starts_with(execution) {
        execution
    } else {
        evidence
    };
    let snapshot = request_configuration::configuration_files(base, environment, Some(declared))?;
    let merged = table(&directory)?;
    let table = serde_json::to_value(&merged)?;
    let endpoint = crate::recovery::inputs::domain::endpoint(
        &request["authority"][format!("{role}_endpoint")],
        "恢复角色端点",
        Some("/readyz"),
    )?;
    let (host, port) = if role == "api" {
        (&table["app"]["host"], &table["app"]["port"])
    } else {
        (&table["jobs"]["health_host"], &table["jobs"]["health_port"])
    };
    let id = table
        .get("snowflake_worker_id")
        .and_then(Value::as_i64)
        .filter(|id| (0..=1023).contains(id))
        .ok_or("恢复角色实例编号无效")?;
    check(
        table["scope_id"].as_str() == private["scope_id"].as_str()
            && table["jobs"]["mode"].as_str() == Some("external")
            && table["monitor"]["metrics_bearer_token"].as_str()
                == private["metrics_bearer_token"].as_str()
            && endpoint.scheme == "http"
            && host.as_str() == Some(endpoint.host)
            && port.as_i64() == Some(i64::from(endpoint.port)),
        "恢复角色配置与 scope、端点、令牌或实例编号不同",
    )?;
    check(
        request_configuration::configuration_files(base, environment, Some(declared))? == snapshot,
        "恢复运行角色配置在读取期间变化",
    )?;
    let (directory, files) = snapshot;
    let digest = digest::plan_hash(&json!({"APP_ENV":"test"}))?;
    let mut result = json!({"directory":directory,"files":files,"environment_sha256":digest});
    if crate::recovery::b0::matches(&request["authority"]) {
        result["legacy"] = legacy_runtime::recipe(&legacy::project(merged)?);
    }
    Ok((id, result))
}

fn table(directory: &Path) -> Result<toml::Table> {
    for name in ["app.toml", "app.test.toml"] {
        super::reads::optional(&directory.join(name))?;
    }
    ryframe_config::AppConfig::read_document(directory, ryframe_config::Environment::Test)
        .map_err(|_| "恢复角色 TOML 配置无法安全加载".into())
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/dataset_authority/environment.rs"]
pub(in crate::recovery::runtime) mod tests;
