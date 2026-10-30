//! 保留服务历史的路径、描述和前后复核；精确编码共用lifecycle state能力。
use super::evidence::{binding, check, exact, text};
use crate::{Result, recovery::clone::native::lifecycle::state};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn count(run: &Path, descriptor: &Value) -> Result<usize> {
    exact(descriptor, &["path", "bytes", "sha256"])?;
    check(
        super::evidence::path_is(&descriptor["path"], &run.join("state.json"))
            && descriptor["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
        "历史阶段记录描述不属于固定运行目录",
    )?;
    let expected = text(descriptor, "sha256")?;
    check(
        expected.len() == 64
            && expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "历史阶段记录摘要无效",
    )?;
    let path = run.join("state.json");
    let before = binding(&path)?;
    let value = state::load(run, true)?;
    let raw = std::fs::read(&path)?;
    check(
        Some(raw.len() as u64) == before["bytes"].as_u64()
            && hex::encode(Sha256::digest(&raw)) == text(&before, "sha256")?
            && serde_json::from_slice::<Value>(&raw)? == value,
        "阶段记录在历史前缀核对期间变化",
    )?;
    let count = state::prefix::count(&raw, descriptor["bytes"].as_u64().unwrap(), expected)?;
    check(binding(&path)? == before, "阶段记录在历史前缀核对期间变化")?;
    Ok(count)
}
