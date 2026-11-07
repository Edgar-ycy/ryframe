use super::protocol::check;
use crate::{Result, recovery::clone::native::json as native_json};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

pub(super) const TOOLS: &[&str] = &[
    "mysql",
    "mysqldump",
    "rustfs",
    "redis_server",
    "wsl",
    "redis_python",
];
const WINDOWS_TOOLS: &[&str] = &["mysql", "mysqldump", "rustfs", "redis_server"];

fn tool_names(tools: &Value) -> Vec<&'static str> {
    if tools["redis_server"].get("platform").is_some() {
        WINDOWS_TOOLS.to_vec()
    } else if tools.get("native_tool").is_some() {
        TOOLS
            .iter()
            .copied()
            .filter(|name| *name != "redis_python")
            .chain(["native_tool"])
            .collect()
    } else {
        // 旧预检仅保留原登记字段的只读核验。
        TOOLS.to_vec()
    }
}

pub(super) fn fixture(value: &Value) -> Result<()> {
    check(
        value["format_version"].as_u64() == Some(1)
            && value["fixture"] == "business"
            && value["status"] == "ready"
            && value["sources"].as_object().is_some_and(|sources| {
                sources
                    .values()
                    .all(|item| item.is_object() && item["head"].is_string())
            })
            && value["paths"].as_object().is_some_and(|paths| {
                paths.keys().map(String::as_str).collect::<BTreeSet<_>>()
                    == BTreeSet::from(["backend", "frontend"])
            }),
        "业务 crate 隔离工作树收据不完整或尚未就绪",
    )
}

pub(super) fn maintenance(value: &Value) -> Result<()> {
    check(
        value["format_version"].as_u64() == Some(1)
            && value["kind"] == "devex-clone-tool-build"
            && value["resources_modified"] == false
            && value["artifacts"].is_object(),
        "维护构建收据不完整",
    )?;
    for role in ["reset", "migrate", "tenant-data"] {
        check(
            value["artifacts"][role].is_object()
                && value["artifacts"][role]["executable"].is_string(),
            "维护构建产物描述无效",
        )?;
    }
    Ok(())
}

pub(super) fn reset_password(value: &Value) -> Result<()> {
    let valid = value["admin_password"].as_str().is_some_and(|password| {
        let bytes = password.as_bytes();
        (8..=72).contains(&bytes.len())
            && bytes.iter().all(|byte| (b'!'..=b'~').contains(byte))
            && bytes.iter().any(u8::is_ascii_uppercase)
            && bytes.iter().any(u8::is_ascii_lowercase)
            && bytes.iter().any(u8::is_ascii_digit)
            && bytes.iter().any(|byte| !byte.is_ascii_alphanumeric())
    });
    check(valid, "admin_password 不满足 reset 管理员密码复杂度策略")
}

pub(super) fn preflight_binding(review: &Value) -> Result<Value> {
    let message = "夹具审阅计划缺少当前工具预检收据";
    let tools = &review["tools"];
    let tool_object = tools.as_object().ok_or(message)?;
    let names = tool_names(tools);
    check(
        names.iter().all(|name| tool_object.contains_key(*name)),
        message,
    )?;
    if tools["redis_server"].get("platform").is_some() {
        super::super::review::validate_windows_tools(tool_object)?;
    } else if tools.get("native_tool").is_some() {
        crate::recovery::clone::native::cache::validate_native_tool(&tools["native_tool"])?;
    }
    let expected = Value::Object(
        names
            .iter()
            .map(|name| ((*name).into(), tools[*name].clone()))
            .collect(),
    );
    let preflight = &review["preflight"];
    check(
        native_json::exact(
            preflight,
            &["format_version", "kind", "status", "supersedes", "tools"],
            "预检",
        )
        .is_ok()
            && native_json::exact(
                &preflight["supersedes"],
                &["path", "bytes", "sha256"],
                "预检来源",
            )
            .is_ok()
            && preflight["format_version"].as_u64() == Some(1)
            && preflight["kind"] == "reference-fixture-tool-preflight"
            && preflight["status"] == "verified"
            && preflight["tools"] == expected,
        message,
    )?;
    let seed_scope = review["scopes"]["seed"]["scope_id"]
        .as_str()
        .ok_or(message)?;
    let run = service_run(review)?;
    let process = review["services"]["rustfs"]["process_receipt"]
        .as_str()
        .map(Path::new)
        .ok_or(message)?;
    check(
        review["services"]["rustfs"]["scope_id"] == format!("services-{seed_scope}")
            && process.is_absolute()
            && process == run.join("rustfs").join("process.json"),
        message,
    )?;
    Ok(expected)
}

pub(super) fn service_run(review: &Value) -> Result<std::path::PathBuf> {
    let value = super::super::services::lifecycle::query::dispatch(
        Path::new("."),
        "service-run",
        &json!({"review":review}),
    )?;
    Ok(std::path::PathBuf::from(
        value.as_str().ok_or("service-run 返回值无效")?,
    ))
}

fn semantics(review: &Value) -> Result<Value> {
    let mut value = review.clone();
    value
        .as_object_mut()
        .ok_or("审阅计划不是对象")?
        .remove("ready_for_execution");
    let tools = value["tools"]
        .as_object_mut()
        .ok_or("审阅计划工具定义无效")?;
    for role in TOOLS.iter().copied().chain(["native_tool"]) {
        tools.remove(role);
    }
    value
        .as_object_mut()
        .expect("已检查对象")
        .remove("preflight");
    Ok(value)
}

pub(super) fn successor(previous: &Value, review: &Value) -> Result<()> {
    let pending = previous["ready_for_execution"] == false && previous.get("preflight").is_none();
    let ready = previous["ready_for_execution"] == true && previous.get("preflight").is_some();
    check(
        pending || ready,
        "preflight predecessor 必须是 pending 或已验证 ready 审阅",
    )?;
    if ready {
        preflight_binding(previous)?;
    }
    let mut structural = previous.clone();
    structural["ready_for_execution"] = json!(true);
    super::super::review::validate_review(&structural)?;
    super::super::review::validate_review(review)?;
    preflight_binding(review)?;
    if ready {
        check(
            previous["tools"]["redis_server"]["platform"]
                == review["tools"]["redis_server"]["platform"],
            "已就绪 Redis 审阅不能切换工具平台；请新建隔离场景",
        )?;
    }
    let expected = semantics(previous)?;
    let mut observed = semantics(review)?;
    if pending {
        let rustfs = observed["services"]["rustfs"]
            .as_object_mut()
            .ok_or("RustFS 定义无效")?;
        rustfs.remove("scope_id");
        rustfs.remove("process_receipt");
    }
    check(
        expected == observed,
        "ready 审阅与其 preflight predecessor 语义不同",
    )
}
