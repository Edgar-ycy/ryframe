//! 冻结生成请求与完整前像守卫；只读核验不授权启动、恢复或重放。
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{bridge, json as document, maintenance, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/generation/build_tests.rs"]
mod build_tests;
#[path = "generation/builds.rs"]
pub(crate) mod builds;
#[path = "generation/coordinator.rs"]
pub(crate) mod coordinator;
#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/generation/tests.rs"]
mod tests;

pub(super) fn dispatch(coordinator: &Path, run: &Path, args: &Value) -> Result<Value> {
    document::exact(args, &["backend_root", "parameters"], "生成守卫参数")?;
    let backend = Path::new(document::text(&args["backend_root"], "生成根目录")?);
    if backend != coordinator {
        validate_local_test_path(backend, coordinator, LocalTestPathKind::ExistingDirectory)?;
    }
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    let parameters = common::read_bound(backend, &args["parameters"], "生成守卫请求快照")?;
    document::exact(&parameters, &["operation", "arguments"], "生成守卫请求快照")?;
    let input = &parameters["arguments"];
    if parameters["operation"] == "coordinator" {
        let value = coordinator::dispatch(input)?;
        common::bound_file(backend, &args["parameters"], "生成守卫请求快照")?;
        return Ok(json!({"verified":true,"remote_writes":0,
            "automatic_retry_permitted":false,"value":value}));
    }
    match parameters["operation"].as_str() {
        Some("builds") => {
            document::exact(input, &["previous", "request"], "生成构建参数")?;
            builds::verify(backend, &input["previous"], &input["request"])?;
        }
        Some("archived-request") => {
            document::exact(
                input,
                &["previous", "request", "segment"],
                "生成冻结请求参数",
            )?;
            let previous = common::read_bound(backend, &input["previous"], "冻结生成请求")?;
            if archived_request(&previous, &input["request"], &input["segment"])? {
                builds::verify(backend, &previous, &input["request"])?;
            }
            common::bound_file(backend, &input["previous"], "冻结生成请求")?;
        }
        Some("archived-image") => {
            document::exact(input, &["before", "after", "segment"], "生成完整前像参数")?;
            if let Err(error) = archived_image(&input["before"], &input["after"], &input["segment"])
            {
                let reason = match error.to_string().as_str() {
                    "未启动恢复的新完整逻辑基线之后存在未知写入；禁止启动" => {
                        "baseline_drift"
                    }
                    "C69 授权后完整逻辑像发生变化；禁止执行 C70 启动" => {
                        "replay_image_drift"
                    }
                    "分段缓存重启改变了配置、端点、二进制或物理目标" => {
                        "redis_generation_changed"
                    }
                    _ => return Err(error),
                };
                return Ok(
                    json!({"verified":false,"reason":reason,"remote_writes":0,"automatic_retry_permitted":false}),
                );
            }
        }
        Some("lineage-receipt") => {
            document::exact(input, &["receipt"], "谱系授权收据参数")?;
            let value = common::read_bound(backend, &input["receipt"], "谱系授权收据")?;
            lineage_receipt(&value)?;
            common::bound_file(backend, &input["receipt"], "谱系授权收据")?;
        }
        _ => return Err("未知生成守卫操作".into()),
    }
    common::bound_file(backend, &args["parameters"], "生成守卫请求快照")?;
    Ok(json!({"verified":true,"remote_writes":0,"automatic_retry_permitted":false}))
}

pub(crate) fn lineage_receipt(value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "status",
            "request",
            "failed_start",
            "failure_proof",
            "history_length",
            "history_sha256",
            "coordinator_source",
            "source_registration",
            "source_rebind",
            "review_successor",
            "current_storage",
            "runtime",
            "before",
            "after",
            "evidence_manifest",
            "source_generation_published",
            "replay_allowed",
            "remote_writes",
            "restore_qualified",
        ],
        "C69 谱系恢复结果",
    )?;
    for field in ["failed_start", "history_length", "remote_writes"] {
        if value[field].as_u64().is_none() {
            return Err("C69 谱系恢复结果的计数字段类型无效".into());
        }
    }
    for field in [
        "source_generation_published",
        "replay_allowed",
        "restore_qualified",
    ] {
        document::boolean(&value[field], "C69 谱系恢复结果布尔字段")?;
    }
    let manifest = &value["evidence_manifest"];
    document::exact(manifest, &["files", "bytes", "sha256"], "C69 完整目录摘要")?;
    if ["files", "bytes"]
        .iter()
        .any(|field| manifest[field].as_u64().is_none_or(|n| n == 0))
        || !manifest["sha256"].as_str().is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    {
        return Err("C69 谱系恢复完整目录摘要无效".into());
    }
    Ok(())
}

/// 返回是否需要复核成对构建；任何其他字段变化均在读取产物前拒绝。
pub(in crate::recovery) fn archived_request(
    previous: &Value,
    request: &Value,
    segment: &Value,
) -> Result<bool> {
    document::object(previous, "冻结生成请求")?;
    document::object(request, "当前生成请求")?;
    let mut expected = previous.clone();
    let mut changed = false;
    if !segment.is_null() {
        if segment["phase"] != "ready" || segment["storage"].is_null() || segment["cache"].is_null()
        {
            return Err("分段资源尚未全部恢复，不能执行实际 START".into());
        }
        expected["current_storage"] = segment["storage"].clone();
        let runtime = request["backend_build"] != previous["backend_build"];
        let maintenance = request["maintenance_build"] != previous["maintenance_build"];
        if runtime != maintenance {
            return Err("运行与维护构建收据必须成对重新登记".into());
        }
        changed = runtime;
        if changed {
            expected["backend_build"] = request["backend_build"].clone();
            expected["maintenance_build"] = request["maintenance_build"].clone();
        }
    }
    if request != &expected {
        return Err("后续实际启动只允许更新当前存储及等价的成对构建收据".into());
    }
    Ok(changed)
}

pub(in crate::recovery) fn archived_image(
    before: &Value,
    after: &Value,
    segment: &Value,
) -> Result<()> {
    document::object(before, "生成冻结完整像")?;
    document::object(after, "生成当前完整像")?;
    let mut expected = before.clone();
    if !segment.is_null() {
        // 重启代次有独立的已声明写入规则，不能误当成普通零写入恢复。
        if !segment["reboot"].is_null() {
            return Err("主机重启完整像必须使用既有声明写入核验入口".into());
        }
        let redis = document::object(&segment["cache"]["redis"], "恢复缓存代次")?;
        let old = document::object(&before["storage"]["redis"], "冻结缓存代次")?;
        if redis.keys().ne(old.keys())
            || old.iter().any(|(key, value)| {
                !["pid", "started", "run_id"].contains(&key.as_str())
                    && redis.get(key) != Some(value)
            })
        {
            return Err("分段缓存重启改变了配置、端点、二进制或物理目标".into());
        }
        expected["storage"] = json!({"rustfs":segment["storage"]["storage"],"redis":redis});
    }
    if &expected != after {
        return Err("未启动恢复的新完整逻辑基线之后存在未知写入；禁止启动".into());
    }
    if !segment["replay"].is_null() && after != &segment["replay"]["image"] {
        return Err("C69 授权后完整逻辑像发生变化；禁止执行 C70 启动".into());
    }
    Ok(())
}
