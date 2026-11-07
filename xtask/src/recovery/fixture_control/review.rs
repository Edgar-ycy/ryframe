use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::{
    Result,
    cli::{FixtureReviewCommand, FixtureReviewOptions},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

use super::{
    file_binding, path_text_canonical, portable_path, print_json, read_json, text_field, write_new,
};

const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;
const KEYS: &[(&str, &str, &str)] = &[
    ("shared-control", "combined", "shared"),
    ("shared", "tenant", "shared"),
    ("dedicated-a", "tenant", "dedicated"),
    ("dedicated-b", "tenant", "dedicated"),
];
const SIDES: &[&str] = &["seed", "base", "candidate"];
const FILE_TOOLS: &[&str] = &["mysql", "mysqldump", "rustfs"];
pub(super) const LINUX_REDIS_TOOLS: &[&str] = &["wsl", "native_tool", "redis_python"];

pub(super) fn run(command: &FixtureReviewCommand, backend: &Path) -> Result<()> {
    let FixtureReviewCommand::Renew(options) = command else {
        println!(
            "cargo xtask check recovery fixture review --template <文件> --execution-source <文件> \
             --future-root <新目录> --id <ID> --api-port <端口> --worker-port <端口> \
             --frontend-port <端口> --rustfs-api-port <端口> \
             --rustfs-console-port <端口> --redis-port <端口> --output <新文件> --write"
        );
        return Ok(());
    };
    let value = renew(backend, options)?;
    print_json(&value)
}

pub(crate) fn renew(backend: &Path, options: &FixtureReviewOptions) -> Result<Value> {
    renew_observed(backend, options, || {})
}

pub(crate) fn renew_observed(
    backend: &Path,
    options: &FixtureReviewOptions,
    observe: impl FnOnce(),
) -> Result<Value> {
    let backend = portable_path(
        &fs::canonicalize(backend)
            .map_err(|error| format!("无法解析当前后端 {}：{error}", backend.display()))?,
    );
    validate_inputs(options, &backend)?;
    let template_file = portable_path(&options.template)
        .components()
        .collect::<PathBuf>();
    let publication_file = portable_path(&options.execution_source)
        .components()
        .collect::<PathBuf>();
    let template = read_controlled_json(&template_file, "审阅模板")?;
    let template_binding = file_binding(&template_file, "审阅模板")?;
    validate_review(&template)?;
    reject_historical_databases(&template)?;
    let publication_binding = file_binding(&publication_file, "执行源码发布收据")?;
    let publication = super::source_pair::execution::verify(&backend, &publication_binding)?;
    let execution = PathBuf::from(text_field(
        &publication["published"]["backend"],
        "path",
        "执行源码",
    )?);
    validate_future_root(&options.future_root, &execution)?;
    let revised = derive(
        &backend,
        &template,
        &publication,
        &execution,
        &template_binding,
        &publication_binding,
        options,
    )?;
    validate_review(&ready_structural(&revised)?)?;
    observe();
    verify_inputs(
        &template_file,
        &template_binding,
        &template,
        &backend,
        &publication_binding,
        &publication,
    )?;
    validate_output(&options.output, &backend)?;
    write_new(&options.output, &revised)?;
    if read_json(&options.output, "审阅续签输出", MAX_RECEIPT_BYTES)? != revised {
        return Err("审阅续签输出发布后无法重算".into());
    }
    Ok(json!({
        "status": "renewed",
        "ready_for_execution": false,
        "remote_writes": 0,
    }))
}

fn validate_inputs(options: &FixtureReviewOptions, backend: &Path) -> Result<()> {
    if !regex::Regex::new(r"^[a-z0-9][a-z0-9-]{2,31}$")?.is_match(&options.id) {
        return Err("新审阅计划 ID 只能使用小写字母、数字和连字符".into());
    }
    for (path, label) in [
        (&options.template, "--template"),
        (&options.execution_source, "--execution-source"),
    ] {
        validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
            .map_err(|error| format!("{label}无效：{error}"))?;
    }
    validate_local_test_path(
        &options.future_root,
        backend,
        LocalTestPathKind::NewDirectory,
    )
    .map_err(|error| format!("--future-root 无效：{error}"))?;
    validate_local_test_path(&options.output, backend, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("--output 无效：{error}"))?;
    validate_ports(options)
}

fn validate_ports(options: &FixtureReviewOptions) -> Result<()> {
    let values = [
        options.api_port,
        options.worker_port,
        options.frontend_port,
        options.rustfs_api_port,
        options.rustfs_console_port,
        options.redis_port,
    ];
    if values.iter().any(|value| !(1024..=65533).contains(value)) {
        return Err("端口基数必须在 1024 至 65533 之间".into());
    }
    let mut expanded = Vec::with_capacity(12);
    for base in &values[..3] {
        expanded.extend([*base, *base + 1, *base + 2]);
    }
    expanded.extend_from_slice(&values[3..]);
    expanded.sort_unstable();
    if expanded.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("三侧 API、Worker、前端以及 RustFS、Redis 端口不能重叠".into());
    }
    Ok(())
}

pub(super) fn validate_review(review: &Value) -> Result<()> {
    let root = review.as_object().ok_or("审阅计划结构无效")?;
    if !root.get("reference").is_some_and(Value::is_object)
        || root.get("kind").and_then(Value::as_str)
            != Some("review-only-perf-resource-plan-with-readonly-preflight")
    {
        return Err("审阅计划类型无效".into());
    }
    if root.get("ready_for_execution") != Some(&Value::Bool(true)) {
        return Err("审阅计划尚未就绪".into());
    }
    let scopes = root
        .get("scopes")
        .and_then(Value::as_object)
        .ok_or("审阅计划必须完整声明三侧资源")?;
    if scopes.keys().map(String::as_str).collect::<BTreeSet<_>>() != SIDES.iter().copied().collect()
    {
        return Err("审阅计划必须完整声明三侧资源".into());
    }
    let tools = root
        .get("tools")
        .and_then(Value::as_object)
        .ok_or("审阅计划缺少工具或服务定义")?;
    let services = root
        .get("services")
        .and_then(Value::as_object)
        .ok_or("审阅计划缺少工具或服务定义")?;
    validate_tools(tools)?;
    validate_services(services)?;
    let reference = root["reference"]
        .as_object()
        .expect("已检查 reference 对象");
    for role in ["source", "protected_target"] {
        let side = reference
            .get(role)
            .and_then(Value::as_object)
            .ok_or("审阅计划保护资源定义无效")?;
        if !side.get("databases").is_some_and(Value::is_array) {
            return Err("审阅计划保护资源定义无效".into());
        }
    }
    for side in scopes.values() {
        validate_scope(side)?;
    }
    Ok(())
}

fn validate_tools(tools: &Map<String, Value>) -> Result<()> {
    for role in FILE_TOOLS {
        let tool = tools
            .get(*role)
            .and_then(Value::as_object)
            .ok_or("审阅计划工具定义无效")?;
        if tool.get("path").and_then(Value::as_str).is_none()
            || tool.get("sha256").and_then(Value::as_str).is_none()
        {
            return Err("审阅计划工具定义无效".into());
        }
    }
    let redis = tools
        .get("redis_server")
        .and_then(Value::as_object)
        .ok_or("审阅计划 Redis 工具定义无效")?;
    if redis.contains_key("platform") {
        return validate_windows_tools(tools);
    }
    if ["distribution", "resolved_path", "sha256"]
        .iter()
        .any(|field| redis.get(*field).and_then(Value::as_str).is_none())
    {
        return Err("审阅计划 Redis 工具定义无效".into());
    }
    Ok(())
}

pub(super) fn validate_windows_tools(tools: &Map<String, Value>) -> Result<()> {
    if LINUX_REDIS_TOOLS.iter().any(|key| tools.contains_key(*key)) {
        return Err("Windows Redis 审阅不能混用 WSL 或 Linux Redis 工具绑定".into());
    }
    let redis = tools
        .get("redis_server")
        .ok_or("审阅计划缺少 Windows Redis 工具")?;
    crate::recovery::clone::native::cache::windows_tool::validate(redis)
}

fn validate_services(services: &Map<String, Value>) -> Result<()> {
    let rustfs = services
        .get("rustfs")
        .and_then(Value::as_object)
        .ok_or("审阅计划服务定义无效")?;
    let cache = services
        .get("redis")
        .and_then(Value::as_object)
        .ok_or("审阅计划服务定义无效")?;
    if ["api", "console", "data_dir"]
        .iter()
        .any(|field| rustfs.get(*field).and_then(Value::as_str).is_none())
        || cache.get("directory").and_then(Value::as_str).is_none()
    {
        return Err("审阅计划服务定义无效".into());
    }
    Ok(())
}

fn validate_scope(value: &Value) -> Result<()> {
    let side = value.as_object().ok_or("审阅计划 scope 无效")?;
    let scope = side
        .get("scope_id")
        .and_then(Value::as_str)
        .ok_or("审阅计划 scope 无效")?;
    if !regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?.is_match(scope) {
        return Err("审阅计划 scope 无效".into());
    }
    for field in [
        "runtime_dir",
        "identity_ledger",
        "api_url",
        "worker_ready_url",
        "frontend_url",
    ] {
        if side.get(field).and_then(Value::as_str).is_none() {
            return Err("审阅计划运行路径无效".into());
        }
    }
    let objects = side
        .get("objects")
        .and_then(Value::as_object)
        .ok_or("审阅计划对象存储定义无效")?;
    if objects.get("endpoint").and_then(Value::as_str).is_none()
        || objects.get("region").and_then(Value::as_str).is_none()
    {
        return Err("审阅计划对象存储定义无效".into());
    }
    let redis = side
        .get("redis")
        .and_then(Value::as_object)
        .ok_or("审阅计划 Redis 定义无效")?;
    for field in ["url", "namespace", "ownership_key", "ownership_value"] {
        if redis.get(field).and_then(Value::as_str).is_none() {
            return Err("审阅计划 Redis 定义无效".into());
        }
    }
    let databases = side
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("审阅计划数据库定义无效")?;
    let keys = databases
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|item| item.get("key").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    if keys != KEYS.iter().map(|(key, _, _)| *key).collect()
        || databases.iter().any(|item| {
            !item.is_object()
                || item.get("database").and_then(Value::as_str).is_none()
                || item
                    .get("expected_server_uuid")
                    .and_then(Value::as_str)
                    .is_none()
        })
    {
        return Err("审阅计划数据库定义无效".into());
    }
    Ok(())
}

fn reject_historical_databases(review: &Value) -> Result<()> {
    let reference = &review["reference"];
    for role in ["source", "protected_target"] {
        if !reference[role]["databases"]
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            return Err("续签模板不得复用历史来源或受保护目标数据库".into());
        }
    }
    Ok(())
}

fn validate_future_root(root: &Path, execution: &Path) -> Result<()> {
    let allowed = execution.join(".local-tests/reference-fixture");
    if root.exists()
        || fs::symlink_metadata(root).is_ok()
        || root.parent() != Some(allowed.as_path())
        || !allowed.is_dir()
    {
        return Err("新运行根目录必须是业务 crate 参考夹具目录下尚不存在的直接子目录".into());
    }
    Ok(())
}

fn derive(
    backend: &Path,
    template: &Value,
    publication: &Value,
    execution: &Path,
    template_binding: &Value,
    execution_binding: &Value,
    options: &FixtureReviewOptions,
) -> Result<Value> {
    let future_root = &options.future_root;
    let plan_id = &options.id;
    let mut revised = template.clone();
    let root = revised.as_object_mut().ok_or("审阅计划结构无效")?;
    root.insert("id".into(), Value::String(format!("fixture-{plan_id}")));
    root.insert(
        "created_at".into(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)),
    );
    root.insert(
        "future_root".into(),
        Value::String(path_text_lexical(future_root)?),
    );
    root.insert(
        "workspace".into(),
        Value::String(path_text_canonical(backend, "当前后端")?),
    );
    root.insert(
        "sources".into(),
        json!({
            "backend_head": publication["original"]["source_pair"]["sources"]["backend"]["head"],
            "frontend_head": publication["original"]["source_pair"]["sources"]["frontend"]["head"],
            "renewed_from": template_binding,
        }),
    );
    root.insert("execution_source".into(), execution_binding.clone());
    root.insert("ready_for_execution".into(), Value::Bool(false));
    let mut scopes = Map::new();
    for (index, side) in SIDES.iter().enumerate() {
        scopes.insert(
            (*side).into(),
            scope(template, side, execution, options, u16::try_from(index)?)?,
        );
    }
    root.insert("scopes".into(), Value::Object(scopes));
    root.remove("preflight");
    let services = root
        .get_mut("services")
        .and_then(Value::as_object_mut)
        .ok_or("审阅计划服务定义无效")?;
    let rustfs = services
        .get_mut("rustfs")
        .and_then(Value::as_object_mut)
        .ok_or("审阅计划服务定义无效")?;
    rustfs.insert(
        "api".into(),
        Value::String(format!("http://127.0.0.1:{}", options.rustfs_api_port)),
    );
    rustfs.insert(
        "console".into(),
        Value::String(format!("http://127.0.0.1:{}", options.rustfs_console_port)),
    );
    rustfs.insert(
        "data_dir".into(),
        Value::String(path_text_lexical(&future_root.join("rustfs"))?),
    );
    rustfs.remove("scope_id");
    rustfs.remove("process_receipt");
    let cache = services
        .get_mut("redis")
        .and_then(Value::as_object_mut)
        .ok_or("审阅计划服务定义无效")?;
    cache.insert(
        "endpoint".into(),
        Value::String(format!("127.0.0.1:{}", options.redis_port)),
    );
    cache.insert(
        "directory".into(),
        Value::String(path_text_lexical(&future_root.join("redis"))?),
    );
    Ok(revised)
}

fn scope(
    result: &Value,
    side: &str,
    execution: &Path,
    options: &FixtureReviewOptions,
    offset: u16,
) -> Result<Value> {
    let future_root = &options.future_root;
    let plan_id = &options.id;
    let api_port = options.api_port + offset;
    let worker_port = options.worker_port + offset;
    let frontend_port = options.frontend_port + offset;
    let rustfs_api_port = options.rustfs_api_port;
    let redis_port = options.redis_port;
    let current = result["scopes"][side]
        .as_object()
        .ok_or("审阅计划必须完整声明三侧资源")?;
    let scope_id = format!("fixture-{side}-{plan_id}");
    let database_token = plan_id.replace('-', "_");
    let secrets = execution
        .join(".local-tests")
        .join("reference-fixture")
        .join("secrets")
        .join("mysql-client.cnf");
    let original_databases = current["databases"]
        .as_array()
        .ok_or("审阅计划数据库定义无效")?;
    let mut databases = Vec::new();
    for (key, _, mode) in KEYS {
        let original = original_databases
            .iter()
            .find(|item| item.get("key").and_then(Value::as_str) == Some(key))
            .ok_or("审阅计划数据库定义无效")?;
        databases.push(json!({
            "key": key,
            "database": format!(
                "ryframe_fx_{database_token}_{side}_{}",
                key.replace('-', "_")
            ),
            "mode": mode,
            "expected_server_uuid": original["expected_server_uuid"],
            "connection_file": path_text_lexical(&secrets)?,
            "host": original["host"],
            "port": original["port"],
        }));
    }
    Ok(json!({
        "scope_id": scope_id,
        "runtime_dir": path_text_lexical(&future_root.join(format!("runtime-{side}")))?,
        "backend_dir": path_text_lexical(execution)?,
        "identity_ledger": path_text_lexical(&future_root.join(format!("identities-{side}")))?,
        "api_url": format!("http://127.0.0.1:{api_port}"),
        "worker_ready_url": format!("http://127.0.0.1:{worker_port}/readyz"),
        "frontend_url": format!("http://127.0.0.1:{frontend_port}"),
        "objects": {
            "endpoint": format!("http://127.0.0.1:{rustfs_api_port}"),
            "region": current["objects"]["region"],
        },
        "redis": {
            "url": format!("redis://127.0.0.1:{redis_port}/0"),
            "namespace": format!("ryframe:{{{scope_id}}}:"),
            "ownership_key": format!("ryframe:{{{scope_id}}}:.ryframe-owner"),
            "ownership_value": format!("ryframe-owner:v1:{scope_id}:redis"),
        },
        "databases": databases,
    }))
}

fn ready_structural(value: &Value) -> Result<Value> {
    let mut structural = value.clone();
    structural["ready_for_execution"] = Value::Bool(true);
    Ok(structural)
}

fn verify_inputs(
    template_file: &Path,
    template_binding: &Value,
    template: &Value,
    backend: &Path,
    publication_binding: &Value,
    publication: &Value,
) -> Result<()> {
    if file_binding(template_file, "审阅模板")? != *template_binding
        || read_controlled_json(template_file, "审阅模板")? != *template
        || super::source_pair::execution::verify(backend, publication_binding)? != *publication
    {
        return Err("审阅续签期间模板、发布收据或完整来源发生变化".into());
    }
    Ok(())
}

fn validate_output(output: &Path, backend: &Path) -> Result<()> {
    validate_local_test_path(output, backend, LocalTestPathKind::OutputFile)
        .map_err(|error| format!("审阅计划输出无效：{error}"))?;
    if !output.parent().is_some_and(Path::is_dir) {
        return Err("新审阅计划输出父目录不存在".into());
    }
    Ok(())
}

fn read_controlled_json(path: &Path, label: &str) -> Result<Value> {
    read_json(path, label, MAX_RECEIPT_BYTES)
}

fn path_text_lexical(path: &Path) -> Result<String> {
    let normalized = portable_path(path).components().collect::<PathBuf>();
    Ok(super::path_text(&normalized, "路径")?.to_owned())
}
