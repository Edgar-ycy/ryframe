//! 仅从固定收据核对 RustFS 原代次移交，避免读取后继复制账本的可变状态。
use std::path::Path;

use serde_json::{Value, json};

use super::{
    evidence::{binding, bound, check, controller, exact, members, path_is, text},
    generation,
};
use crate::Result;

pub(in crate::recovery) struct Selected {
    pub(in crate::recovery) tree: Value,
    pub(in crate::recovery) tree_binding: Value,
    pub(in crate::recovery) rustfs_request: Value,
    pub(in crate::recovery) request_binding: Value,
    pub(in crate::recovery) redis_runtime: Value,
    pub(in crate::recovery) redis_binding: Value,
}

impl Selected {
    pub(in crate::recovery) fn matches_redis(&self, binding: &Value) -> Result<bool> {
        same_binding(binding, &self.redis_binding)
    }
}

fn same_binding(recorded: &Value, selected: &Value) -> Result<bool> {
    exact(recorded, &["path", "bytes", "sha256"])?;
    let path = Path::new(text(selected, "path")?);
    Ok(path_is(&recorded["path"], path)
        && recorded["bytes"] == selected["bytes"]
        && recorded["sha256"] == selected["sha256"])
}

/// 只使用已通过完整历史校验的当前代次，继续读取该代次的固定收据。
pub(in crate::recovery) fn selected(run: &Path, generation: &Value) -> Result<Selected> {
    let (rustfs, redis) = generation::paths(run, generation)?;
    let tree_path = rustfs.join("rustfs-tree.json");
    let request_path = rustfs.join("request.json");
    let redis_path = redis.join("runtime.json");
    let tree_binding = binding(&tree_path)?;
    let request_binding = binding(&request_path)?;
    let redis_binding = binding(&redis_path)?;
    let tree = bound(run, &tree_binding)?;
    let rustfs_request = bound(run, &request_binding)?;
    let redis_runtime = bound(run, &redis_binding)?;
    if generation["kind"] != "initial" {
        let outer = bound(run, generation)?;
        let current = bound(run, &outer["generation"])?;
        let storage = &current["rustfs"]["storage"];
        check(
            same_binding(&storage["tree"], &tree_binding)?
                && storage["identity"] == tree["process"]
                && same_binding(&current["rustfs"]["request"], &request_binding)?
                && current["redis"]["runtime"] == redis_runtime,
            "RustFS 移交未绑定已发布的当前服务后继收据",
        )?;
    }
    Ok(Selected {
        tree,
        tree_binding,
        rustfs_request,
        request_binding,
        redis_runtime,
        redis_binding,
    })
}

pub(super) fn validate(
    run: &Path,
    number: usize,
    item: &Value,
    result: &Value,
    history: &mut Value,
) -> Result<()> {
    check(
        history["closed"] == false
            && history["initial_complete"] == true
            && history["external_recovery"].is_null()
            && history["unsettled"].as_array().is_some_and(Vec::is_empty)
            && history["rustfs_handoff"].is_null()
            && result["status"] == "rustfs_handed_off"
            && result["services"] == json!({"redis":"running","rustfs":"handed-off"})
            && result["controller"] == controller(run, item)?,
        "RustFS 移交必须是唯一且明确收尾的已登记代次移交",
    )?;
    check(
        result["seed_close"] == item["sources"]["seed_close"]
            && result["copy_state"] == item["sources"]["copy_state"],
        "RustFS 移交结果与原复制来源绑定不同",
    )?;
    exact(&result["seed_close"], &["path", "bytes", "sha256"])?;
    exact(&result["copy_state"], &["path", "bytes", "sha256"])?;
    let seed = Path::new(text(&result["seed_close"], "path")?);
    let copy = seed
        .parent()
        .and_then(Path::parent)
        .ok_or("种子关闭缺少复制账本")?;
    check(
        copy != run
            && copy.is_absolute()
            && seed.parent() == Some(copy.join("results").as_path())
            && path_is(&result["copy_state"]["path"], &copy.join("state.json"))
            && result["seed_close"] == binding(seed)?,
        "RustFS 移交的种子关闭结果已变更或不属于原复制账本",
    )?;
    exact(&result["evidence"], &["rustfs", "redis"])?;
    let rustfs = &result["evidence"]["rustfs"];
    let redis = &result["evidence"]["redis"];
    let selected = selected(run, &history["active_generation"])?;
    check(
        path_is(
            &rustfs["path"],
            &run.join(format!("lifecycle-{number:04}/rustfs-stopped.json")),
        ) && selected.matches_redis(redis)?,
        "RustFS 移交证据不属于原代次与本阶段",
    )?;
    let proof = bound(run, rustfs)?;
    let cache = bound(run, redis)?;
    check(
        proof["tree"] == selected.tree
            && proof["data_directory"] == selected.rustfs_request["data_directory"]
            && proof["terminated"] == true
            && cache["format_version"] == 3
            && proof["redis"]["process"] == cache["process"]
            && proof["redis"]["run_id"] == cache["redis"]["run_id"]
            && proof["redis"]["markers_sha256"]
                .as_str()
                .is_some_and(|sha| {
                    sha.len() == 64
                        && sha
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }),
        "RustFS 移交未绑定同一数据目录、完整原树及 Redis 代次",
    )?;
    members(&proof, run)?;
    history["rustfs_handoff"] = item["result"].clone();
    Ok(())
}
