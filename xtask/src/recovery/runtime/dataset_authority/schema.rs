//! 恢复运行 v3 的严格形状与内嵌权威校验。
use super::{check, path};
use crate::{
    Result,
    recovery::{
        clone::native::{json as strict, resources::common},
        inputs::{document, domain},
    },
};
use serde_json::Value;

pub(in crate::recovery::runtime) fn runtime(value: &Value) -> Result<()> {
    strict::exact(
        value,
        &[
            "format_version",
            "kind",
            "restore",
            "paths",
            "digests",
            "source",
            "endpoints",
            "backend",
            "frontend",
            "processes",
        ],
        "恢复运行收据",
    )?;
    check(
        value["format_version"] == 3 && value["kind"] == "restore-runtime",
        "恢复运行收据版本或类型不同",
    )?;
    strict::exact(
        &value["restore"],
        &[
            "id",
            "backup_id",
            "plan_hash",
            "scope_id",
            "data_verified_at",
        ],
        "恢复身份",
    )?;
    for key in ["id", "backup_id"] {
        domain::identifier(strict::text(&value["restore"][key], "恢复 ID")?)?;
    }
    domain::scope_identifier(strict::text(&value["restore"]["scope_id"], "恢复 scope")?)?;
    document::digest(&value["restore"]["plan_hash"], "恢复计划摘要")?;
    domain::timestamp(&value["restore"]["data_verified_at"], "恢复数据校验时间")?;
    strict::exact(
        &value["paths"],
        &[
            "backend_product_root",
            "backend_execution_root",
            "frontend_root",
            "runtime_dir",
            "bindings",
            "backend_build",
            "frontend_build",
            "launch",
        ],
        "恢复路径",
    )?;
    for value in strict::object(&value["paths"], "恢复路径")?.values() {
        path(value)?;
    }
    strict::exact(
        &value["digests"],
        &["bindings", "backend_build", "frontend_build", "launch"],
        "恢复摘要",
    )?;
    for value in strict::object(&value["digests"], "恢复摘要")?.values() {
        document::digest(value, "恢复输入摘要")?;
    }
    strict::exact(
        &value["source"],
        &[
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
        ],
        "恢复源码",
    )?;
    for key in [
        "backup_source_sha",
        "backend_product_sha",
        "backend_execution_sha",
        "frontend_sha",
    ] {
        domain::hash_40(&value["source"][key], "恢复精确来源")?;
    }
    let same = value["source"]["backend_product_sha"] == value["source"]["backend_execution_sha"];
    check(
        if same {
            value["source"]["backend_adapter_contract"].is_null()
        } else {
            value["source"]["backend_adapter_contract"] == "legacy-stable-readiness-b0-v1"
        },
        "恢复产品、执行来源与适配合同不同",
    )?;
    endpoints(&value["endpoints"])?;
    processes(&value["processes"])
}

pub(in crate::recovery::runtime) fn endpoints(value: &Value) -> Result<()> {
    strict::exact(value, &["api", "worker", "frontend"], "恢复端点")?;
    let mut ports = std::collections::BTreeSet::new();
    for role in ["api", "worker", "frontend"] {
        let suffix = if role == "frontend" { "" } else { "/readyz" };
        let endpoint = domain::endpoint(&value[role], "恢复角色端点", Some(suffix))?;
        let host = if endpoint.host.contains(':') {
            format!("[{}]", endpoint.host)
        } else {
            endpoint.host.to_owned()
        };
        let canonical = format!("{}://{host}:{}{suffix}", endpoint.scheme, endpoint.port);
        check(
            value[role] == canonical && ports.insert(endpoint.port),
            "恢复角色端点必须规范且使用互异显式端口",
        )?;
    }
    Ok(())
}
fn processes(value: &Value) -> Result<()> {
    strict::exact(value, &["api", "worker", "frontend"], "恢复进程")?;
    for role in ["api", "worker", "frontend"] {
        let item = &value[role];
        strict::exact(
            item,
            &["receipt_path", "receipt_sha256", "identity"],
            "恢复进程绑定",
        )?;
        path(&item["receipt_path"])?;
        document::digest(&item["receipt_sha256"], "恢复进程收据摘要")?;
        common::validate_process_identity(&item["identity"])?;
    }
    Ok(())
}
