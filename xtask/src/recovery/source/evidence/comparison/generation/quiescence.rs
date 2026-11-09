//! 停止证明只读核验；不补写完成收据、不探测服务或发送控制命令。
use super::super::{common, document};
use crate::{
    Result,
    recovery::{
        fixture_control::services::lifecycle::readonly::NativeRuntime, source::evidence::running,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn verify(
    backend: &Path,
    output: &Path,
    request: &Value,
    effective: &Value,
) -> Result<Value> {
    running::directory(backend, output)?;
    let build = running::builds::verify(backend, request)?;
    let value = document::read(&output.join("runtime-evidence.json"))?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "intent",
            "runtime",
            "roles",
            "physical_binding",
            "ports_idle",
            "observed_stopped_at",
        ],
        "源停止证明",
    )?;
    let observed = chrono::DateTime::parse_from_rfc3339(document::text(
        &value["observed_stopped_at"],
        "源停止时间",
    )?)?;
    if observed > chrono::Utc::now() {
        return Err("源停止观察不能来自未来".into());
    }
    let intent = common::read_bound(backend, &value["intent"], "源启动意图")?;
    document::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "request",
            "runtime_directory",
            "operations",
        ],
        "源启动意图",
    )?;
    let runtime = output.join("runtime");
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "seed-source-runtime-quiescence"
        || intent["format_version"].as_u64() != Some(1)
        || intent["kind"] != "seed-source-runtime-intent"
        || intent["request"] != common::binding(&output.join("request.json"))?
        || intent["runtime_directory"] != json!(runtime)
        || value["runtime"] != effective["runtime"]
        || value["ports_idle"] != json!(["api", "worker"])
    {
        return Err("源停止证明缺少同一请求、启动意图或完整静止角色".into());
    }
    document::exact(&value["roles"], &["api", "worker"], "源停止双角色")?;
    running::roles::directory(&runtime, &intent["operations"])?;
    let contract = common::read_bound(backend, &effective["runtime"], "后继 runtime")?;
    if contract["backend_root"] != request["execution_backend"] {
        return Err("后继 runtime 执行根与来源请求不同".into());
    }
    for role in ["api", "worker"] {
        if contract["artifacts"][role]
            != json!({"path":build["artifacts"][role]["executable"],"sha256":build["artifacts"][role]["sha256"]})
        {
            return Err("后继 runtime 未绑定同一 v2 产物".into());
        }
        let row = &value["roles"][role];
        document::exact(
            row,
            &["process", "tree", "completion", "ready"],
            "源停止角色",
        )?;
        let tree = running::roles::tree(&runtime, role, &effective["source"]["scope_id"])?;
        if tree["operation_id"] != intent["operations"][role]
            || row["tree"] != common::binding(&runtime.join(format!("{role}-tree.json")))?
            || row["process"] != effective["processes"][role]
            || row["ready"] != true
            || tree["process"]
                != running::roles::process(&runtime, role, &effective["source"]["scope_id"])?
        {
            return Err("源停止进程树、创建身份或 ready 绑定变化".into());
        }
        // 复用已封存完成证明的只读验证入口，不创建新的 completion。
        if NativeRuntime::new(backend).complete_registered_tree(&tree)? != row["completion"] {
            return Err("源停止完整成员证明与已绑定完成收据不同".into());
        }
    }
    if document::read(&output.join("runtime-evidence.json"))? != value {
        return Err("源停止证明在核验期间变化".into());
    }
    Ok(value)
}
