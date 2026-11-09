//! close checkpoint 的摘要、控制器前像与阶段结果绑定。
use super::result;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document, resources::common},
};
use serde_json::Value;
use std::path::Path;

pub(in crate::recovery::clone::native::seed) fn validate(
    backend: &Path,
    input: &Value,
) -> Result<Value> {
    document::exact(
        input,
        &[
            "directory",
            "attempt",
            "value",
            "phase",
            "previous",
            "runtime",
            "handoff",
        ],
        "关闭 checkpoint 参数",
    )?;
    let directory = Path::new(document::text(&input["directory"], "关闭 run")?);
    let runtime = Path::new(document::text(&input["runtime"], "关闭运行目录")?);
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)?;
    validate_local_test_path(runtime, backend, LocalTestPathKind::ExistingDirectory)?;
    let value = &input["value"];
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "attempt",
            "controller",
            "handoff",
            "runtime_directory",
            "scope_id",
            "phase",
            "previous",
            "resumed_from",
            "result",
            "sha256",
        ],
        "关闭 checkpoint",
    )?;
    let number = input["attempt"]["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("关闭阶段序号无效")?;
    let phase = document::text(&input["phase"], "关闭 checkpoint 阶段")?;
    let mut body = value.clone();
    body.as_object_mut()
        .ok_or("关闭 checkpoint 不是对象")?
        .remove("sha256");
    let handoff_path = runtime.join("handoff.json");
    let handoff = common::binding(&handoff_path)?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-seed-close-checkpoint"
        || value["attempt"].as_u64() != Some(number)
        || value["phase"] != input["phase"]
        || value["previous"] != input["previous"]
        || value["sha256"].as_str() != Some(digest::plan_hash(&body)?.as_str())
        || value["handoff"] != handoff
        || value["runtime_directory"].as_str() != runtime.to_str()
        || value["scope_id"] != input["handoff"]["scope_id"]
        || document::read(&handoff_path)? != input["handoff"]
    {
        return Err("close checkpoint 不属于固定阶段或摘要链".into());
    }
    controller(backend, directory, &input["attempt"], &value["controller"])?;
    result::validate(&value["result"], runtime, &input["handoff"], &handoff, true)?;
    if value["result"]["phase"] != phase {
        return Err("close checkpoint 阶段与结果不同".into());
    }
    if input["previous"].is_null() {
        if value["resumed_from"].is_null() && !["running", "api_stopped"].contains(&phase) {
            return Err("close checkpoint 中途阶段缺少固定续作来源".into());
        }
    } else if !value["resumed_from"].is_null() {
        return Err("close checkpoint 只有链首可以绑定续作来源".into());
    }
    common::bound_file(backend, &value["controller"], "关闭控制器")?;
    if common::binding(&handoff_path)? != handoff {
        return Err("checkpoint 核验期间交接绑定变化".into());
    }
    Ok(value["result"].clone())
}

fn controller(backend: &Path, directory: &Path, attempt: &Value, descriptor: &Value) -> Result<()> {
    let path = common::bound_file(backend, descriptor, "关闭控制器")?;
    let value = document::read(&path)?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "owner",
            "attempt",
            "attempt_sha256",
        ],
        "关闭控制器",
    )?;
    document::exact(
        &value["owner"],
        &["format_version", "identity", "directory", "manifest_sha256"],
        "关闭所有者",
    )?;
    let number = attempt["number"].as_u64().ok_or("关闭阶段序号无效")?;
    let mut original = attempt.clone();
    original["status"] = Value::String("running".into());
    for field in ["finished_at", "result", "error_type"] {
        original[field] = Value::Null;
    }
    let manifest = common::binding(&directory.join("manifest.json"))?;
    if path != directory.join(format!("controller-{number:04}.json"))
        || value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-stage-controller"
        || value["attempt"].as_u64() != Some(number)
        || value["attempt_sha256"].as_str() != Some(digest::plan_hash(&original)?.as_str())
        || value["owner"]["format_version"].as_u64() != Some(1)
        || value["owner"]["directory"].as_str() != directory.to_str()
        || value["owner"]["manifest_sha256"] != manifest["sha256"]
    {
        return Err("close checkpoint 控制器不属于固定 attempt".into());
    }
    super::close::identity(&value["owner"]["identity"])?;
    Ok(())
}
