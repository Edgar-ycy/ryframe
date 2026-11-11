//! 仅从固定收据核对 RustFS 原代次移交，避免读取后继复制账本的可变状态。
use std::path::Path;

use serde_json::{Value, json};

use super::evidence::{binding, bound, check, controller, exact, members, path_is, read, text};
use crate::Result;

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
            && history["active_generation"]["kind"] == "initial"
            && history["external_recovery"].is_null()
            && history["unsettled"].as_array().is_some_and(Vec::is_empty)
            && history["rustfs_handoff"].is_null()
            && result["status"] == "rustfs_handed_off"
            && result["services"] == json!({"redis":"running","rustfs":"handed-off"})
            && result["controller"] == controller(run, item)?,
        "RustFS 移交必须是唯一且明确收尾的首代移交",
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
    check(
        path_is(
            &rustfs["path"],
            &run.join(format!("lifecycle-{number:04}/rustfs-stopped.json")),
        ) && path_is(&redis["path"], &run.join("redis/runtime.json")),
        "RustFS 移交证据不属于原代次与本阶段",
    )?;
    let proof = bound(run, rustfs)?;
    let cache = bound(run, redis)?;
    check(
        proof["tree"] == read(&run.join("rustfs/rustfs-tree.json"))?
            && proof["data_directory"] == read(&run.join("rustfs/request.json"))?["data_directory"]
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
