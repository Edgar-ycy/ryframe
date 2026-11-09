use super::source;
use crate::{
    Result,
    recovery::clone::native::{digest, json as json_io, lifecycle::state},
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn validate(registered: &Value) -> Result<&Value> {
    json_io::exact(registered, &["attempt", "controller"], "登记 attempt")?;
    let attempt = &registered["attempt"];
    json_io::exact(
        attempt,
        &[
            "number",
            "stage",
            "mode",
            "started_at",
            "finished_at",
            "status",
            "sources",
            "result",
            "error_type",
        ],
        "完整 attempt",
    )?;
    let mode = attempt["mode"].as_str().ok_or("attempt mode 无效")?;
    let status = attempt["status"].as_str().ok_or("attempt status 无效")?;
    if attempt["number"].as_u64().is_none_or(|n| n == 0)
        || !state::STAGES.contains(&attempt["stage"].as_str().unwrap_or(""))
        || !mode.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || !mode
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || ["started_at", "finished_at"]
            .iter()
            .any(|key| attempt[*key].as_str().is_none_or(str::is_empty))
        || !attempt["sources"].is_object()
        || !matches!(status, "passed" | "failed")
        || (status == "passed" && !attempt["error_type"].is_null())
        || (status == "failed" && !attempt["error_type"].is_string())
    {
        return Err("Node attempt 必须是已收尾的完整阶段记录".into());
    }
    if !attempt["result"].is_null() {
        json_io::exact(
            &attempt["result"],
            &["path", "bytes", "sha256"],
            "attempt 结果",
        )?;
    }
    Ok(attempt)
}
pub(super) fn controller(run: &Path, attempt: &Value, expected: &Value) -> Result<Value> {
    let number = attempt["number"].as_u64().ok_or("attempt number 无效")?;
    let path = run.join(format!("controller-{number:04}.json"));
    let binding = source::bound(&path, run, Some(expected))?;
    let value = json_io::read(&path)?;
    json_io::exact(
        &value,
        &[
            "format_version",
            "kind",
            "owner",
            "attempt",
            "attempt_sha256",
        ],
        "历史控制器",
    )?;
    let owner = &value["owner"];
    json_io::exact(
        owner,
        &["format_version", "identity", "directory", "manifest_sha256"],
        "控制器 owner",
    )?;
    source::identity(&owner["identity"], None)?;
    let manifest = source::bound(&run.join("manifest.json"), run, None)?;
    let mut original = attempt.clone();
    original["status"] = json!("running");
    for key in ["finished_at", "result", "error_type"] {
        original[key] = Value::Null;
    }
    if value["format_version"] != 1
        || value["kind"] != "devex-stage-controller"
        || value["attempt"] != number
        || value["attempt_sha256"] != digest::plan_hash(&original)?
        || owner["format_version"] != 1
        || owner["directory"] != json!(run)
        || owner["manifest_sha256"] != manifest["sha256"]
    {
        return Err("Node controller 与 attempt 不一致".into());
    }
    source::bound(&path, run, Some(&binding))?;
    Ok(binding)
}
pub(super) fn owner(process: &Value, kind: &str) -> Result<()> {
    let key = if kind.starts_with("identity-") {
        "identity_lock_owner"
    } else {
        "producer_owner"
    };
    let mut fields = vec!["format_version", "kind", "launch", "identity"];
    if process.get(key).is_some() {
        fields.push(key);
        let text = process[key].as_str().ok_or("生产者 owner 无效")?;
        if text.len() != 36
            || uuid::Uuid::parse_str(text).ok().is_none_or(|id| {
                id.get_version_num() != 4 || id.get_variant() != uuid::Variant::RFC4122
            })
            || !text.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err("原生生产者 owner 格式无效".into());
        }
    } else if kind != "existing" {
        return Err("原生生产者收据必须记录 owner".into());
    }
    json_io::exact(process, &fields, "生产者进程收据")?;
    Ok(())
}
