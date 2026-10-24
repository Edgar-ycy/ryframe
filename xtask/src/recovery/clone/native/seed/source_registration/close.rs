//! 选择原 run 最新已通过的 close，并复核完整排空结果及之后的阶段边界。
use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        json as document, lifecycle::state, resources::common, seed::protocol::guards,
    },
};

const ALLOWED_AFTER_CLOSE: [(&str, &str); 21] = [
    ("seed-runtime", "source-register"),
    ("seed-runtime", "source-rebind"),
    ("seed-runtime", "source-generation-start"),
    ("seed-runtime", "source-generation-stop"),
    ("seed-runtime", "source-generation-recover"),
    ("seed-runtime", "source-export"),
    ("seed-runtime", "source-export-reconcile"),
    ("seed-runtime", "arm-input"),
    ("seed-runtime", "stop"),
    ("seed-runtime", "recover"),
    ("seed-runtime", "recover-session"),
    ("storage-target", "restart"),
    ("storage-target", "status"),
    ("storage-target", "stop"),
    ("storage-target", "recover"),
    ("cache-target", "restart"),
    ("cache-target", "status"),
    ("cache-target", "stop"),
    ("cache-target", "recover"),
    ("cache-target", "reconcile"),
    ("cache-target", "resume"),
];

pub(crate) fn latest(
    backend: &Path,
    run: &Path,
    state: &Value,
    runtime: &Path,
    handoff: &Value,
    current: u64,
) -> Result<Value> {
    common::local_directory(backend, run, "seed source-register 运行目录")?;
    let attempts = state::attempts(state)?;
    let record = attempts
        .iter()
        .rfind(|item| {
            item["stage"] == "seed-runtime"
                && item["mode"] == "close"
                && item["number"].as_u64() != Some(current)
        })
        .ok_or("最新 seed close 尚未成功发布")?;
    if record["status"] != "passed" {
        return Err("最新 seed close 尚未成功发布，不能回退历史成功".into());
    }
    let number = record["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or("seed close 阶段序号无效")?;
    let descriptor = record.get("result").ok_or("seed close 缺少外层结果绑定")?;
    let path = common::bound_file(backend, descriptor, "seed close 外层结果")?;
    if path != run.join("results").join(format!("{number:04}.json")) {
        return Err("seed close 外层结果不属于固定 run attempt".into());
    }
    let value = document::read(&path)?;
    guards::bound_result(
        backend,
        &json!({"value":value,"runtime":runtime,"handoff":handoff,"in_progress":false}),
    )?;
    if value["status"] != "seed_runtime_closed" || value["outbox_drained"] != true {
        return Err("seed close 未取得完整 outbox 排空终态".into());
    }
    for attempt in attempts {
        let later = attempt["number"].as_u64().ok_or("seed 历史阶段序号无效")?;
        if number < later && later < current {
            let pair = (
                document::text(&attempt["stage"], "seed 后续阶段")?,
                document::text(&attempt["mode"], "seed 后续模式")?,
            );
            if !ALLOWED_AFTER_CLOSE.contains(&pair) {
                return Err("seed close 后出现未重新排空的阶段，不能登记复制源".into());
            }
        }
    }
    if common::bound_file(backend, descriptor, "seed close 外层结果")? != path
        || common::binding(&path)? != *descriptor
    {
        return Err("seed close 结果在读取期间变化；保留现场，禁止自动重放".into());
    }
    Ok(descriptor.clone())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/close.rs"]
mod tests;
