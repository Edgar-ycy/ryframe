//! 将启动配置与目标计划的私有环境及实际 TOML 快照交叉核验。
use super::{check, inputs::Inputs, path};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as strict, target_verify::input::request_configuration,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(super) fn verify(inputs: &mut Inputs, request: &Value, target: &Value) -> Result<()> {
    let binding = &target["fresh_target"]["environment"];
    check(
        request["environment"]["document"] == *binding,
        "恢复启动环境未绑定目标私有环境",
    )?;
    let document = inputs.bound(binding)?;
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
    for role_name in ["api", "worker"] {
        let id = role(
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
    }
    Ok(())
}

fn role(
    request: &Value,
    private: &Value,
    execution: &Path,
    evidence: &Path,
    environment: &BTreeMap<String, String>,
    role: &str,
) -> Result<i64> {
    let declared = &private[format!("{role}_config_dir")];
    let directory = path(declared)?;
    // 配置可位于执行根内，或协调器本次私有环境文件的证据目录内。

    let base = if directory.starts_with(execution) {
        execution
    } else {
        evidence
    };
    let (actual_directory, files) =
        request_configuration::configuration_files(base, environment, Some(declared))?;
    let expected = json!({"directory":actual_directory,"files":files,"environment_sha256":digest::plan_hash(&json!({"APP_ENV":"test"}))?});
    check(
        request["environment"]["configurations"][role] == expected,
        "恢复启动角色配置快照发生漂移",
    )?;
    let table = table(&directory)?;
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

    Ok(id)
}

fn table(directory: &Path) -> Result<Value> {
    let mut merged = toml::Table::new();
    for name in ["app.toml", "app.test.toml"] {
        let path = directory.join(name);
        super::reads::optional(&path)?;
        if name == "app.toml" && !path.try_exists()? {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let table: toml::Table = toml::from_str(&text).map_err(|_| "恢复角色 TOML 配置无法解析")?;
        for (key, value) in table {
            if let (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) =
                (merged.get_mut(&key), &value)
            {
                existing.extend(incoming.clone());
            } else {
                merged.insert(key, value);
            }
        }
    }
    Ok(serde_json::to_value(toml::Value::Table(merged))?)
}
