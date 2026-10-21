//! 运行测量的纯配置与 operation 目录校验，不读取环境或运行资源。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

const STRUCTURE: &str = "运行时配置结构无效";
const ENDPOINT: &str = "测量端点必须为不含凭据、query 和 fragment 的 HTTP URL";
const METRICS: &str = "进程指标必须且只能登记 api 与 worker";
const DATABASE_METRICS: &str = "必须登记隔离 MySQL 实例的连接数指标端点";
const WORKFLOWS: &str = "运行时场景集合不完整或重复";
const JOB_TIMINGS: &str = "任务场景缺少明确的隔离控制库、服务器身份与 MySQL 客户端绑定";
const CATALOG: &str = "operation 目录结构无效";

/// 校验运行输入的纯配置契约，源码指纹和 authority 绑定由调用层负责。
pub(crate) fn validate(config: &Value, suite: &str) -> Result<(), String> {
    if !config.is_object()
        || config["schema_version"].as_f64() != Some(1.0)
        || !config["contract"].is_object()
        || !config["bindings"].is_object()
    {
        return Err(STRUCTURE.into());
    }
    let contract = &config["contract"];
    let bindings = &config["bindings"];
    if !bindings["scope_id"].as_str().is_some_and(valid_scope) {
        return Err("必须登记隔离 scope".into());
    }
    if !digest(&contract["dataset_sha256"]) || !digest(&contract["environment_sha256"]) {
        return Err("缺少固定数据集或硬件、存储、网络与服务参数指纹".into());
    }
    for key in ["api_url", "frontend_url"] {
        http_url(&bindings[key])?;
    }
    metrics(bindings)?;
    super::runtime_pacing::validate_contract(&contract["pacing"]).map_err(str::to_owned)?;
    bounded(&contract["cycles"], 10, 10_000, "每个场景的请求周期")?;
    bounded(&contract["timeout_ms"], 1000, 300_000, "请求超时")?;
    if bindings.get("identities").is_some()
        || bindings["identity_pools"]
            .as_object()
            .is_none_or(|pools| pools.is_empty())
    {
        return Err("缺少独立身份池或仍使用旧 identities 结构".into());
    }
    if suite != "homepage" {
        workflows(contract, bindings, suite)?;
    }
    Ok(())
}

fn metrics(bindings: &Value) -> Result<(), String> {
    let metrics = &bindings["metrics_urls"];
    if !exact(metrics, &["api", "worker"]) {
        return Err(METRICS.into());
    }
    if http_url(&metrics["api"])? == http_url(&metrics["worker"])? {
        return Err("API 与 Worker 指标端点规范化后不能重复".into());
    }
    let databases = bindings["database_metrics_urls"]
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or(DATABASE_METRICS)?;
    let mut selected = BTreeSet::new();
    for endpoint in databases {
        if !selected.insert(http_url(endpoint)?) {
            return Err("数据库指标端点规范化后不能重复".into());
        }
    }
    Ok(())
}

fn workflows(contract: &Value, bindings: &Value, suite: &str) -> Result<(), String> {
    let required: &[&str] = match suite {
        "api" => &["list", "filter", "page", "login-refresh", "write"],
        "jobs" => &["export", "import", "message", "schedule"],
        "tenants" => &["list", "write", "export"],
        _ => return Err(WORKFLOWS.into()),
    };
    let workflows = contract["workloads"][suite].as_array().ok_or(WORKFLOWS)?;
    let mut names = BTreeSet::new();
    for workflow in workflows {
        let name = workflow["name"].as_str().ok_or(WORKFLOWS)?;
        if !names.insert(name) {
            return Err(WORKFLOWS.into());
        }
    }
    if workflows.len() != required.len() || required.iter().any(|name| !names.contains(name)) {
        return Err(WORKFLOWS.into());
    }
    for workflow in workflows {
        if workflow["steps"]
            .as_array()
            .is_none_or(|steps| !(1..=30).contains(&steps.len()))
        {
            return Err("场景必须包含 1 到 30 个实际操作".into());
        }
        if suite == "jobs" && workflow["job"]["kind"] != workflow["name"] {
            return Err("任务场景必须声明同种类的精确关联任务采集".into());
        }
        if truthy(&workflow["job"])
            && !exact(
                &bindings["job_timings"],
                &["control", "mysql_client", "server_uuid"],
            )
        {
            return Err(JOB_TIMINGS.into());
        }
    }
    Ok(())
}

/// 从已解析契约提取 operation；不拼接前缀或推断 HTTP 方法。
pub(crate) fn operation_catalog(document: &Value) -> Result<BTreeMap<String, Value>, String> {
    if !document["x-ryframe-api-prefix"]["value"]
        .as_str()
        .is_some_and(absolute_path_start)
    {
        return Err("契约缺少明确的 API 前缀".into());
    }
    let paths = document["paths"].as_object().ok_or(CATALOG)?;
    let mut operations = BTreeMap::new();
    for (route, item) in paths {
        for (method, operation) in item.as_object().ok_or(CATALOG)? {
            if operation.is_null() {
                return Err(CATALOG.into());
            }
            let identifier = &operation["operationId"];
            if !truthy(identifier) {
                continue;
            }
            let identifier = identifier.as_str().ok_or("operation ID 必须是字符串")?;
            if operations.contains_key(identifier) {
                return Err("重复 operation ID".into());
            }
            if !absolute_path_start(route) || route.contains(['?', '#']) {
                return Err("operation 必须声明当前服务的绝对路径".into());
            }
            operations.insert(
                identifier.to_owned(),
                json!({"method": method.to_uppercase(), "path": route}),
            );
        }
    }
    Ok(operations)
}

fn absolute_path_start(value: &str) -> bool {
    value.starts_with('/') && !value.starts_with("//")
}

fn valid_scope(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn http_url(value: &Value) -> Result<String, String> {
    let text = value.as_str().ok_or(ENDPOINT)?;
    let url = url::Url::parse(text).map_err(|_| ENDPOINT.to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some_and(|value| !value.is_empty())
        || url.query().is_some_and(|value| !value.is_empty())
        || url.fragment().is_some_and(|value| !value.is_empty())
    {
        return Err(ENDPOINT.into());
    }
    Ok(url.to_string())
}

fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn bounded(value: &Value, minimum: u64, maximum: u64, name: &str) -> Result<(), String> {
    let number = value.as_f64().ok_or_else(|| format!("{name} 超出范围"))?;
    if !number.is_finite()
        || number.fract() != 0.0
        || !(0.0..=9_007_199_254_740_991.0).contains(&number)
        || !(minimum as f64..=maximum as f64).contains(&number)
    {
        return Err(format!("{name} 超出范围"));
    }
    Ok(())
}

fn exact(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
