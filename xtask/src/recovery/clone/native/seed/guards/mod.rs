//! close/rebind 的只读模型守卫；不取得资源所有权，不执行启停或远端写入。
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        digest, json as document, lifecycle::state, resources::common, seed::arm_input::history,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::clone::native::seed) mod checkpoint;
pub(in crate::recovery::clone::native::seed) mod close;
pub(crate) mod rebind;
pub(in crate::recovery::clone::native::seed) mod result;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/guards/tests.rs"]
mod tests;

pub(super) fn dispatch(coordinator: &Path, run: &Path, args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["backend_root", "parameters"],
        "close/rebind 守卫参数",
    )?;
    let backend = Path::new(document::text(&args["backend_root"], "守卫根目录")?);
    if backend != coordinator {
        validate_local_test_path(backend, coordinator, LocalTestPathKind::ExistingDirectory)?;
    }
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    let parameters = common::read_bound(backend, &args["parameters"], "close/rebind 请求快照")?;
    document::exact(
        &parameters,
        &["operation", "arguments"],
        "close/rebind 请求快照",
    )?;
    let input = &parameters["arguments"];
    let value = match parameters["operation"].as_str() {
        Some("rebind-model") => rebind::dispatch(input)?,
        Some("arm-prefix-history") => {
            document::exact(input, &["run", "record", "source"], "历史 ARM 前缀参数")?;
            let run = Path::new(document::text(&input["run"], "历史 ARM 运行目录")?);
            validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
            let saved = state::load(run, true)?;
            history::prove(
                backend,
                run,
                &saved,
                &input["record"],
                &input["source"],
                None,
            )?;
            if state::load(run, true)? != saved {
                return Err("历史 ARM 前缀证明期间原账本变化".into());
            }
            Value::Null
        }
        Some("rebind-transition-files") => rebind::live::transition(backend, input)?,
        Some("rebind-quiet-producers") => rebind::live::quiet(backend, input)?,
        Some("rebind-restart-files") => rebind::live::restart(backend, input)?,
        Some("snapshot") => {
            document::exact(input, &["value"], "排空快照参数")?;
            close::snapshot(&input["value"])?;
            input["value"].clone()
        }
        Some("parse-snapshot") => {
            document::exact(input, &["raw"], "排空快照解析参数")?;
            parse_snapshot(document::text(&input["raw"], "排空快照原文")?)?
        }
        Some("identity") => {
            document::exact(input, &["value"], "关闭进程身份参数")?;
            close::identity(&input["value"])?;
            input["value"].clone()
        }
        Some("control") => close::control_request(input)?,
        Some("drain") => {
            document::exact(input, &["value", "worker_identity"], "稳定排空参数")?;
            result::drain(&input["value"], &input["worker_identity"])?;
            input["value"].clone()
        }
        Some("result") => bound_result(backend, input)?,
        Some("checkpoint") => checkpoint::validate(backend, input)?,
        Some("rebind-transition") => {
            document::exact(input, &["original", "current"], "重绑定换代参数")?;
            transition(&input["original"], &input["current"])?;
            Value::Null
        }
        Some("rebind-receipt") => {
            document::exact(
                input,
                &["value", "descriptor", "original", "prefix"],
                "重绑定收据参数",
            )?;
            rebind_receipt(input)?;
            Value::Null
        }
        _ => return Err("未知 close/rebind 模型操作".into()),
    };
    common::bound_file(backend, &args["parameters"], "close/rebind 请求快照")?;
    Ok(json!({"value":value,"remote_writes":0,"automatic_retry_permitted":false}))
}

pub(in crate::recovery::clone::native::seed) fn parse_snapshot(raw: &str) -> Result<Value> {
    let value = document::parse_fragment(raw, "排空快照")?;
    close::snapshot(&value)?;
    Ok(value)
}

fn transition(original: &Value, current: &Value) -> Result<()> {
    if !original.is_object() || !current.is_object() || current == original {
        return Err("重绑定需要不同的已发布存储代次".into());
    }
    for field in ["request", "data_directory", "api_url", "console_url"] {
        if original.get(field).is_none() || current.get(field) != original.get(field) {
            return Err("重绑定不得改变固定请求、目录身份或监听端点".into());
        }
    }
    let old = &original["storage"];
    let new = &current["storage"];
    if old["sha256"].as_str().is_none()
        || old["sha256"] != new["sha256"]
        || old["identity"]["executable"].as_str().is_none()
        || old["identity"]["executable"] != new["identity"]["executable"]
        || !matches!((original["attempt"].as_u64(), current["attempt"].as_u64()),
            (Some(before), Some(after)) if before > 0 && after > before)
    {
        return Err("重绑定不是同一二进制的后续代次".into());
    }
    Ok(())
}

pub(crate) fn rebind_receipt(input: &Value) -> Result<()> {
    let value = &input["value"];
    document::exact(
        value,
        &[
            "status",
            "source_registration",
            "review_successor",
            "original_storage",
            "current_storage",
            "history_length",
            "history_sha256",
            "remote_writes",
            "restore_qualified",
        ],
        "重绑定收据",
    )?;
    let prefix = document::array(&input["prefix"], "重绑定历史前缀")?;
    if value["status"] != "seed_source_rebound"
        || value["source_registration"] != input["descriptor"]
        || value["original_storage"] != input["original"]
        || value["history_length"].as_u64() != Some(prefix.len() as u64)
        || value["history_sha256"].as_str() != Some(digest::plan_hash(&input["prefix"])?.as_str())
        || value["remote_writes"].as_u64() != Some(0)
        || value["restore_qualified"].as_bool() != Some(false)
    {
        return Err("重绑定没有精确冻结来源及完整历史前缀".into());
    }
    Ok(())
}

pub(crate) fn bound_result(backend: &Path, input: &Value) -> Result<Value> {
    document::exact(
        input,
        &["value", "runtime", "handoff", "in_progress"],
        "关闭结果参数",
    )?;
    let runtime = Path::new(document::text(&input["runtime"], "关闭运行目录")?);
    validate_local_test_path(runtime, backend, LocalTestPathKind::ExistingDirectory)?;
    let path = runtime.join("handoff.json");
    let before = common::binding(&path)?;
    if document::read(&path)? != input["handoff"] {
        return Err("关闭交接内容与实际绑定文件不同".into());
    }
    result::validate(
        &input["value"],
        runtime,
        &input["handoff"],
        &before,
        document::boolean(&input["in_progress"], "进行中标志")?,
    )?;
    if common::binding(&path)? != before {
        return Err("关闭结果核验期间 handoff 变化".into());
    }
    Ok(input["value"].clone())
}
