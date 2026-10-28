//! 只读核对已完成清理的失败首代；运行身份只能取自原停止收据。
use super::{
    super::{digest, json as doc, resources::common},
    evidence, model, process,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn validate(request: &Value, output: &Path) -> Result<Value> {
    model::shape(request)?;
    model::plain(output)?;
    let bindings = files(output)?;
    if request["format_version"] != 2
        || output.join("runtime.json").try_exists()?
        || output.join("partial.json").try_exists()?
    {
        return Err("失败清理只接受未发布运行收据的原生首代".into());
    }
    let original = evidence::inspect(request, output)?;
    let failure = doc::read(&output.join("failure.json"))?;
    doc::exact(
        &failure,
        &[
            "error_type",
            "launcher",
            "request_sha256",
            "runtime_recorded",
            "cleanup_completed",
        ],
        "失败清理收据",
    )?;
    let stopped = doc::read(&output.join("failure-stop/stopped.json"))?;
    let exited = doc::read(&output.join("linux-exited.json"))?;
    let runtime = &stopped["runtime"];
    let mut stop_fields = vec![
        "identity",
        "alive",
        "boot_id",
        "terminated",
        "status",
        "runtime",
        "resources_deleted",
    ];
    if stopped.get("observed_after_failed_stop").is_some() {
        if stopped["observed_after_failed_stop"] != true {
            return Err("失败停止后的观察标记无效".into());
        }
        stop_fields.push("observed_after_failed_stop");
    }
    doc::exact(&stopped, &stop_fields, "失败首代停止收据")?;
    process::validate_response(
        &serde_json::to_vec(&json!({"identity":stopped["identity"],
        "alive":stopped["alive"],"boot_id":stopped["boot_id"],"terminated":stopped["terminated"]}))?,
        &original["linux_identity"],
    )?;
    if original.is_null()
        || !original.is_object()
        || failure["error_type"] != "NativeCacheFailure"
        || failure["request_sha256"] != digest::plan_hash(request)?
        || failure["launcher"] != original["launcher"]
        || failure["cleanup_completed"] != true
        || failure["runtime_recorded"] != true
        || stopped["status"] != "redis_process_stopped"
        || stopped["alive"] != false
        || stopped["resources_deleted"] != false
        || !runtime.is_object()
        || runtime.as_object().is_some_and(|fields| {
            fields.keys().any(|key| {
                original.get(key).is_none() && !["redis", "facts"].contains(&key.as_str())
            })
        })
        || doc::read(&output.join("request.json"))? != *request
        || original.as_object().is_none_or(|fields| {
            fields
                .iter()
                .any(|(key, value)| runtime.get(key) != Some(value))
        })
        || exited["identity"] != original["linux_identity"]
        || exited["returncode"].as_i64().is_none()
    {
        return Err("失败首代缺少同请求、原创建身份或完整停止退出证据".into());
    }
    doc::exact(&exited, &["identity", "returncode"], "缓存原身份退出")?;
    if output.join("accepted.json").try_exists()?
        && !output.join("acknowledged.json").try_exists()?
    {
        return Err("缓存接受回执缺少原确认".into());
    }
    for name in ["acknowledged.json", "accepted.json"] {
        if output.join(name).try_exists()? {
            let value = doc::read(&output.join(name))?;
            if value
                != json!({"request_sha256":digest::plan_hash(request)?,"identity":original["linux_identity"]})
            {
                return Err("失败首代授权回执与原身份不同".into());
            }
        }
    }
    if bindings != files(output)? || evidence::inspect(request, output)? != original {
        return Err("失败清理核验期间原证据变化".into());
    }
    Ok(json!({"runtime":runtime,"bindings":bindings}))
}

fn files(output: &Path) -> Result<Value> {
    let required = [
        "request.json",
        "launch-request.json",
        "intent.json",
        "launcher.json",
        "linux-intent.json",
        "linux-process.json",
        "failure.json",
        "linux-exited.json",
        "failure-stop/stopped.json",
    ];
    let optional = [
        "redis.stdout.log",
        "redis.stderr.log",
        "wsl.stdout.log",
        "wsl.stderr.log",
        "acknowledged.json",
        "accepted.json",
    ];
    let mut result = json!({});
    for name in required {
        model::plain(&output.join(name))?;
        result[format!("redis_{name}")] = common::binding(&output.join(name))?;
    }
    for directory in [output.to_path_buf(), output.join("failure-stop")] {
        model::plain(&directory)?;
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            model::plain(&path)?;
            if path == output.join("failure-stop") && entry.file_type()?.is_dir() {
                continue;
            }
            let name = path
                .strip_prefix(output)?
                .to_str()
                .ok_or("失败清理路径不是UTF-8")?
                .replace('\\', "/");
            if !entry.file_type()?.is_file()
                || (!required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
            {
                return Err("失败首代目录含未知文件、链接或写入".into());
            }
            if !name.ends_with(".log") {
                result[format!("redis_{name}")] = common::binding(&path)?;
            }
        }
    }
    Ok(result)
}
