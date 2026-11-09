use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{input, json as document},
};

const REQUEST: &[&str] = &[
    "format_version",
    "kind",
    "source_registration",
    "id",
    "initialized",
    "target_registration",
    "target_initialized_files",
    "target_environment",
    "copy_directory",
    "build_bridges",
];
const RESULT: &[&str] = &[
    "status",
    "request",
    "manifest",
    "source_registration",
    "source_request",
    "source_storage",
    "generation_verified",
    "initialized",
    "target_environment",
    "target_registration",
    "target_initialized_files",
    "target_storage_run",
    "target_side",
    "remote_writes",
    "outbox_drained",
    "restore_qualified",
];
const SUCCESSOR: &[&str] = &[
    "review_successor",
    "source_export",
    "source_export_result",
    "source_generation",
];
pub(super) const SOURCE: &[&str] = &[
    "source_registration",
    "source_rebind",
    "review_successor",
    "source_export",
    "source_export_result",
    "source_generation",
];

pub(super) fn request(value: &Value) -> Result<bool> {
    let successor = match value["kind"].as_str() {
        Some("devex-clone-seed-arm-input") => false,
        Some("devex-clone-seed-successor-arm-input") => true,
        _ => return Err("arm-input 请求类型无效".into()),
    };
    if value["format_version"].as_u64() != Some(1) {
        return Err("arm-input 请求版本无效".into());
    }
    let mut fields = REQUEST.to_vec();
    if successor {
        fields.extend_from_slice(SUCCESSOR);
    }
    if value.get("source_rebind").is_some() {
        fields.push("source_rebind");
    }
    document::exact(value, &fields, "arm-input 请求")?;
    input::name(document::text(&value["id"], "arm 输入 ID")?)?;
    Ok(successor)
}

pub(super) fn check_result(value: &Value, request_value: &Value) -> Result<()> {
    let mut fields = RESULT.to_vec();
    if request(request_value)? {
        fields.extend_from_slice(SUCCESSOR);
    }
    if request_value.get("source_rebind").is_some() {
        fields.push("source_rebind");
    }
    document::exact(value, &fields, "已发布 arm 输入")?;
    if value["status"] != "seed_arm_input_published"
        || value["remote_writes"].as_u64() != Some(0)
        || value["outbox_drained"] != true
        || value["restore_qualified"] != false
        || !matches!(value["target_side"].as_str(), Some("base" | "candidate"))
    {
        return Err("历史 arm-input 外层结果类型或目标侧无效".into());
    }
    check_fields(value, request_value)
}

pub(super) fn check_fields(value: &Value, request: &Value) -> Result<()> {
    for field in SOURCE.iter().copied().chain([
        "initialized",
        "target_registration",
        "target_initialized_files",
        "target_environment",
    ]) {
        // 普通清单保留 source_export:null，普通请求与阶段结果省略它。
        if value[field] != request[field] {
            return Err("历史 arm 输入没有绑定同一来源或原目标文件".into());
        }
    }
    Ok(())
}

pub(super) fn manifest(request: &Value, source: &Value, storage: &Value, copy: &str) -> Value {
    let mut value = json!({
        "format_version":1,"kind":"devex-clone-run","id":request["id"],
        "source_request":source["source_request"],"source_export":request["source_export"],
        "initialized":request["initialized"],"source_environment":source["source_environment"],
        "target_environment":request["target_environment"],"copy_directory":copy,"copy_stage":"seed_to_arm",
        "build_bridges":request["build_bridges"],"source_registration":request["source_registration"],
        "target_registration":request["target_registration"],"target_initialized_files":request["target_initialized_files"],
        "target_storage_run":storage
    });
    for &field in SUCCESSOR.iter().chain(["source_rebind"].iter()) {
        if let Some(binding) = request.get(field) {
            value[field] = binding.clone();
        }
    }
    value
}

pub(super) fn result(
    request: &Value,
    source: &Value,
    storage: &Value,
    side: &str,
    request_binding: &Value,
    manifest_binding: &Value,
) -> Value {
    let mut value = json!({
        "status":"seed_arm_input_published","request":request_binding,"manifest":manifest_binding,
        "source_registration":request["source_registration"],"source_request":source["source_request"],
        "source_storage":source["registration"]["source_storage"],"generation_verified":source["generation_verified"],
        "initialized":request["initialized"],"target_environment":request["target_environment"],
        "target_registration":request["target_registration"],"target_initialized_files":request["target_initialized_files"],
        "target_storage_run":storage,"target_side":side,"remote_writes":0,"outbox_drained":true,"restore_qualified":false
    });
    for &field in SUCCESSOR.iter().chain(["source_rebind"].iter()) {
        if let Some(binding) = request.get(field) {
            value[field] = binding.clone();
        }
    }
    value
}
