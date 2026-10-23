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
    let tools = review["tools"].as_object();
    check(
        tools.is_some_and(|tools| TOOLS.iter().all(|name| tools.contains_key(*name))),
        message,
    )?;
    let expected = Value::Object(
        TOOLS
            .iter()
            .map(|name| ((*name).into(), review["tools"][*name].clone()))
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
    check(
        review["services"]["rustfs"]["scope_id"] == format!("services-{seed_scope}")
            && review["services"]["rustfs"]["process_receipt"]
                == run
                    .join("rustfs")
                    .join("process.json")
                    .to_string_lossy()
                    .as_ref(),
        message,
    )?;
    Ok(expected)
}

fn service_run(review: &Value) -> Result<std::path::PathBuf> {
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
    for role in TOOLS {
        tools.remove(*role);
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
