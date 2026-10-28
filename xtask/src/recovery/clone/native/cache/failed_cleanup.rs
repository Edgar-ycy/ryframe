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
    let directory_present = output.join("failure-stop").try_exists()?;
    if request["format_version"] != 2 || output.join("partial.json").try_exists()? {
        return Err("失败清理只接受证据完整的原生首代".into());
    }
    let original = evidence::inspect(request, output)?;
    let needs_reconciliation =
        doc::read(&output.join("failure.json"))?["cleanup_completed"] == false;
    let runtime = if needs_reconciliation {
        unconfirmed(request, output, &original)?;
        original.clone()
    } else {
        stopped_runtime(request, output, &original)?
    };
    authorization(request, output, &original, &runtime)?;
    linux_failure(request, output, &original)?;
    super::cleanup_diagnostic::sidecar(request, output, &original)?;
    if bindings != files(output)?
        || evidence::inspect(request, output)? != original
        || directory_present != output.join("failure-stop").try_exists()?
    {
        return Err("失败清理核验期间原证据变化".into());
    }
    let mut result = json!({"runtime":runtime,"bindings":bindings});
    if needs_reconciliation {
        result["reconcile_required"] = json!(true);
        result["cleanup_directory_present"] = json!(directory_present);
    }
    Ok(result)
}

fn stopped_runtime(request: &Value, output: &Path, original: &Value) -> Result<Value> {
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
    Ok(runtime.clone())
}

fn authorization(request: &Value, output: &Path, original: &Value, runtime: &Value) -> Result<()> {
    let published = output.join("runtime.json").try_exists()?;
    if published
        && (!output.join("acknowledged.json").try_exists()?
            || !output.join("accepted.json").try_exists()?
            || doc::read(&output.join("runtime.json"))? != *runtime)
    {
        return Err("已发布失败首代缺少完整接受确认或运行收据与原停止身份不同".into());
    }
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
    Ok(())
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
    ];
    let optional = [
        "redis.stdout.log",
        "redis.stderr.log",
        "wsl.stdout.log",
        "wsl.stderr.log",
        "acknowledged.json",
        "accepted.json",
        "runtime.json",
        "linux-failure.json",
        "failure-stop/stopped.json",
        "cleanup-diagnostic.json",
    ];
    let mut result = json!({});
    for name in required {
        model::plain(&output.join(name))?;
        result[format!("redis_{name}")] = common::binding(&output.join(name))?;
    }
    for directory in [output.to_path_buf(), output.join("failure-stop")] {
        if directory == output.join("failure-stop") && !directory.try_exists()? {
            continue;
        }
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

fn unconfirmed(request: &Value, output: &Path, original: &Value) -> Result<()> {
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
        "父清理未确认收据",
    )?;
    if !original.is_object()
        || failure["error_type"] != "NativeCacheFailure"
        || failure["launcher"] != original["launcher"]
        || failure["request_sha256"] != digest::plan_hash(request)?
        || failure["runtime_recorded"] != true
        || failure["cleanup_completed"] != false
        || doc::read(&output.join("request.json"))? != *request
        || !output.join("linux-failure.json").try_exists()?
    {
        return Err("父清理未确认且缺少完整原请求、创建身份或Linux终结证据".into());
    }
    for name in [
        "acknowledged.json",
        "accepted.json",
        "runtime.json",
        "partial.json",
        "failure-stop/stopped.json",
    ] {
        if output.join(name).try_exists()? {
            return Err("父清理未确认分支含接受、运行、停止或其他发布结果".into());
        }
    }
    let directory = output.join("failure-stop");
    if directory.try_exists()? && std::fs::read_dir(&directory)?.next().is_some() {
        return Err("父清理目录含未知结果，不能协调".into());
    }
    let exited = doc::read(&output.join("linux-exited.json"))?;
    doc::exact(&exited, &["identity", "returncode"], "Linux原创建身份退出")?;
    if exited["identity"] != original["linux_identity"] {
        return Err("Linux退出收据不属于原创建身份".into());
    }
    Ok(())
}

fn linux_failure(request: &Value, output: &Path, original: &Value) -> Result<()> {
    let path = output.join("linux-failure.json");
    if !path.try_exists()? {
        return Ok(());
    }
    let failure = doc::read(&path)?;
    doc::exact(
        &failure,
        &[
            "error_type",
            "diagnostic",
            "cleanup_error_type",
            "identity",
            "request_sha256",
            "cleanup_completed",
            "child_pid",
            "child_returncode",
            "popen_called",
            "intent",
        ],
        "Linux失败清理收据",
    )?;
    let diagnostic = super::diagnostic::Diagnostic::decode(&failure["diagnostic"])?;
    let exited = doc::read(&output.join("linux-exited.json"))?;
    let mut intent = common::binding(&output.join("linux-intent.json"))?;
    intent["path"] = json!(model::linux_path(doc::text(
        &intent["path"],
        "Linux启动意图路径"
    )?)?);
    if failure["error_type"] != "NativeCacheFailure"
        || failure["request_sha256"] != digest::plan_hash(request)?
        || failure["intent"] != intent
        || failure["identity"] != original["linux_identity"]
        || failure["child_pid"] != original["linux_identity"]["pid"]
        || failure["popen_called"] != true
        || failure["cleanup_completed"] != true
        || !failure["cleanup_error_type"].is_null()
        || failure["child_returncode"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .is_none()
        || failure["child_returncode"] != exited["returncode"]
        || !matches!(
            diagnostic.write_outcome,
            super::diagnostic::Outcome::Stopped
        )
    {
        return Err("Linux失败清理缺少同请求、原child身份、确定停止或一致退出码".into());
    }
    if matches!(
        diagnostic.category,
        super::diagnostic::Category::AcceptanceTimeout
    ) && !output.join("acknowledged.json").try_exists()?
    {
        return Err("Linux接受复核失败缺少原确认收据".into());
    }
    Ok(())
}
