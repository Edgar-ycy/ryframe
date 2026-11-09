//! 缓存阶段复用完整来源历史；不复制封存、分段或主机证明规则。
use super::{prelaunch, reboot, resolution, segment};
use crate::{
    Result,
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::Value;
use std::path::Path;

pub(crate) fn dispatch(backend: &Path, run: &Path, operation: &str, args: &Value) -> Result<Value> {
    common::local_directory(backend, run, "缓存来源历史目录")?;
    let binding = common::binding(&run.join("state.json"))?;
    let manifest = common::binding(&run.join("manifest.json"))?;
    let saved = state::load(run, true)?;
    let rows = state::attempts(&saved)?;
    let result = match operation {
        "seed-history" => {
            document::exact(args, &["source", "current", "mode"], "缓存来源历史")?;
            resolution::inspect_current(
                backend,
                run,
                &saved,
                &args["source"],
                &args["current"],
                args["current"].is_null()
                    && matches!(
                        args["mode"].as_str(),
                        Some("resume" | "reconcile" | "stop" | "recover")
                    ),
                None,
            )?["later"]
                .clone()
        }
        "seed-segment" => {
            document::exact(args, &["source", "current"], "缓存封存分段")?;
            let archive = prelaunch::closed(backend, run, rows)?;
            if archive.is_null() {
                Value::Null
            } else {
                segment::read(
                    backend,
                    run,
                    rows,
                    &archive,
                    &args["source"],
                    &args["current"],
                )?
            }
        }
        "restart-authorization" => {
            document::exact(args, &["current"], "缓存重启授权")?;
            reboot::cache_restart(backend, run, rows, &args["current"])?
        }
        _ => return Err("未知缓存来源历史端口".into()),
    };
    common::bound_file(backend, &binding, "缓存来源历史后像")?;
    common::bound_file(backend, &manifest, "缓存来源清单后像")?;
    Ok(result)
}
