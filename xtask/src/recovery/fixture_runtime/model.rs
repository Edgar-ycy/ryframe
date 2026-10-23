use crate::{
    Result,
    recovery::clone::native::{digest, json as strict, resources::common},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(super) const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub(super) const MAX_FILES: u64 = 10_000;
pub(super) const MAX_LOG: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Rejected(pub String);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Rejected {}

pub(super) fn check(condition: bool, message: &str) -> Result<()> {
    if !condition {
        return Err(Box::new(Rejected(message.to_owned())));
    }
    Ok(())
}
pub(super) fn text(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| "浏览器证据缺少字符串字段".into())
}
pub(super) fn path(value: &Value) -> Result<PathBuf> {
    let path = PathBuf::from(text(value)?);
    check(path.is_absolute(), "浏览器证据路径必须是绝对路径")?;
    safe(&path)?;
    Ok(path)
}
pub(super) fn safe(path: &Path) -> Result<()> {
    check(
        path.is_absolute()
            && !path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }),
        "浏览器路径必须规范化",
    )?;
    check(
        !digest::linked_path(path)?.contains(&true),
        "浏览器路径不得经过链接或重解析点",
    )
}
pub(super) fn directory(path: &Path) -> Result<()> {
    safe(path)?;
    check(
        fs::symlink_metadata(path)?.is_dir(),
        "浏览器产物不是普通目录",
    )
}
pub(super) fn state(path: &Path) -> Result<Value> {
    safe(path)?;
    let m = fs::symlink_metadata(path)?;
    check(m.is_file(), "浏览器证据必须是普通文件")?;
    let identity = common::device_inode_for(path, &m)?;
    let modified = m
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    Ok(json!({"identity":identity,"bytes":m.len(),"modified":modified}))
}
pub(super) fn snapshot(path: &Path, limit: u64) -> Result<Value> {
    let before = state(path)?;
    check(
        before["bytes"].as_u64().is_some_and(|size| size <= limit),
        "浏览器证据超过字节上限",
    )?;
    let hash = digest::file(path)?;
    check(
        before == state(path)? && before["bytes"] == hash.bytes,
        "浏览器证据读取期间被替换或修改",
    )?;
    Ok(json!({"descriptor":{"path":path,"bytes":hash.bytes,"sha256":hash.sha256},"state":before}))
}
pub(super) fn unchanged(guard: &Value) -> Result<()> {
    let path = path(&guard["descriptor"]["path"])?;
    check(
        snapshot(&path, MAX_BYTES)? == *guard,
        "浏览器证据被替换或修改",
    )
}
pub(super) fn bound(path: &Path) -> Result<Value> {
    Ok(snapshot(path, MAX_BYTES)?["descriptor"].clone())
}
pub(super) fn document(path: &Path) -> Result<(Value, Value)> {
    let guard = snapshot(path, 16 * 1024 * 1024)?;
    let bytes = fs::read(path)?;
    check(
        json!(hex::encode(Sha256::digest(&bytes))) == guard["descriptor"]["sha256"],
        "浏览器JSON读取期间发生内容替换",
    )?;
    let value = strict::parse_fragment(
        std::str::from_utf8(&bytes).map_err(|_| "浏览器证据不是UTF-8")?,
        "浏览器证据",
    )
    .map_err(|_| "浏览器证据不是唯一字段的严格JSON")?;
    check(value.is_object(), "浏览器证据必须是对象")?;
    unchanged(&guard)?;
    Ok((value, guard))
}
pub(super) fn descriptor(value: &Value, path: &Path) -> Result<Value> {
    strict::exact(value, &["path", "bytes", "sha256"], "浏览器文件绑定")?;
    check(*value == bound(path)?, "浏览器文件与绑定摘要不一致")?;
    Ok(value.clone())
}
pub(super) fn write_new(path: &Path, value: &Value) -> Result<()> {
    safe(path)?;
    let mut stream = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)?;
    stream.sync_all()?;
    Ok(())
}
pub(super) fn read_log(path: &Path, descriptor_value: &Value, secrets: &[String]) -> Result<Value> {
    let guard = snapshot(path, MAX_LOG)?;
    descriptor(descriptor_value, path)?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_LOG + 1)
        .read_to_end(&mut bytes)?;
    check(bytes.len() as u64 <= MAX_LOG, "浏览器日志超过上限")?;
    check(
        json!(hex::encode(Sha256::digest(&bytes))) == guard["descriptor"]["sha256"],
        "浏览器日志读取期间发生内容替换",
    )?;
    check(
        !secrets.iter().any(|secret| {
            !secret.is_empty() && bytes.windows(secret.len()).any(|b| b == secret.as_bytes())
        }),
        "浏览器日志包含未脱敏凭据",
    )?;
    unchanged(&guard)?;
    Ok(descriptor_value.clone())
}
