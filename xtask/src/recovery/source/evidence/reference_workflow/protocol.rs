use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{exact, inputs, native, path, text};
use crate::Result;

const BASE: &[&str] = &[
    "backend_dir",
    "format_version",
    "operation",
    "plan",
    "write",
];
const TARGET: &[&str] = &[
    "backup_receipt",
    "comparison_sources",
    "arm_input",
    "fresh_target_verify",
    "product_plan",
];

pub(super) fn request(args: &Value) -> Result<Value> {
    exact(args, &["raw"])?;
    let raw = text(&args["raw"])?;
    if raw.is_empty() || raw.len() > 65_536 {
        return Err("reference私有协议为空或超过64KiB".into());
    }
    let outer = native::json::parse_fragment(raw, "reference私有协议")
        .map_err(|_| "reference私有协议必须为唯一字段的严格JSON")?;
    exact(&outer, &["format_version", "kind", "request"])?;
    if outer["format_version"].as_u64() != Some(1)
        || outer["kind"] != "ryframe-xtask-recovery-reference"
    {
        return Err("reference私有协议版本或类型无效".into());
    }
    let value = &outer["request"];
    let operation = text(&value["operation"])?;
    let extras: &[&str] = match operation {
        "plan" => plan_fields(value)?,
        "check-dataset" | "dataset" => &[],
        "check-existing" => &["side"],
        "backup" => &["inventory", "source_generation", "source_export_result"],
        "restore" => &[
            "backup_root",
            "record",
            "target_plan",
            "runtime_registration",
        ],
        "copy" => &["backup_root", "copy_id"],
        "damage" => &["backup_root", "artifact", "missing"],
        _ => return Err("reference私有协议操作无效".into()),
    };
    let fields = BASE.iter().chain(extras).copied().collect::<Vec<_>>();
    exact(value, &fields)?;
    let writes = matches!(
        operation,
        "dataset" | "backup" | "restore" | "copy" | "damage"
    ) || operation == "plan" && value.get("output").is_some();
    if value["format_version"].as_u64() != Some(1) || value["write"].as_bool() != Some(writes) {
        return Err("reference私有请求版本或写入授权无效".into());
    }
    for field in [
        "backend_dir",
        "plan",
        "inventory",
        "source_generation",
        "source_export_result",
        "backup_root",
        "record",
        "runtime_registration",
        "backup_receipt",
        "comparison_sources",
        "arm_input",
        "fresh_target_verify",
        "product_plan",
        "target_plan",
        "output",
    ] {
        if let Some(value) = value.get(field) {
            path(value)?;
        }
    }
    if operation == "check-existing" && !matches!(value["side"].as_str(), Some("source" | "target"))
    {
        return Err("check-existing side无效".into());
    }
    if operation == "copy" {
        inputs::domain::identifier(text(&value["copy_id"])?)?;
    }
    if operation == "damage" {
        super::super::reference::safe_relative(text(&value["artifact"])?)?;
        native::json::boolean(&value["missing"], "missing")?;
    }
    let mut result = value.as_object().ok_or("reference请求必须为对象")?.clone();
    result.remove("format_version");
    result.remove("operation");
    result.insert("command".into(), json!(operation));
    Ok(Value::Object(result))
}

fn plan_fields(value: &Value) -> Result<&'static [&'static str]> {
    let values = native::json::object(value, "reference plan请求")?;
    let extras = values
        .keys()
        .map(String::as_str)
        .filter(|key| !BASE.contains(key))
        .collect::<BTreeSet<_>>();
    if extras.is_empty() {
        return Ok(&[]);
    }
    if extras == BTreeSet::from(["target_plan"]) {
        return Ok(&["target_plan"]);
    }
    if extras == TARGET.iter().copied().collect() {
        return Ok(TARGET);
    }
    if extras == TARGET.iter().copied().chain(["output"]).collect() {
        return Ok(&[
            "backup_receipt",
            "comparison_sources",
            "arm_input",
            "fresh_target_verify",
            "product_plan",
            "output",
        ]);
    }
    Err("reference plan私有协议的模式或输入组合无效".into())
}

pub(super) fn dataset(args: &Value) -> Result<Value> {
    exact(args, &["backend", "plan", "preflight"])?;
    for field in ["backend", "plan", "preflight"] {
        path(&args[field])?;
    }
    let result = serde_json::to_string(&json!({
        "backend_dir":args["backend"],"format_version":1,
        "kind":"ryframe-xtask-recovery-dataset-prepare","plan":args["plan"],
        "preflight":args["preflight"],"side":"source","verify_existing":null,"write":true
    }))?;
    if result.len() > 32 * 1024 {
        return Err("数据准备私有协议超过32KiB".into());
    }
    Ok(Value::String(result))
}
