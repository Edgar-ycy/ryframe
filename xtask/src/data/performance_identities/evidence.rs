//! 身份证据采用固定 JSON 编码；发布不覆盖，更新先核对完整前像。
use crate::{
    Result,
    recovery::clone::native::{digest, json},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub(super) fn encode(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub(super) fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
pub(super) fn encoded_hash(value: &Value) -> Result<String> {
    Ok(hash(encode(value)?))
}

pub(super) fn regular(path: &Path) -> Result<Option<Vec<u8>>> {
    let before = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !before.is_file() || digest::linked(&before) || before.len() > 16 * 1024 * 1024 {
        return Err("身份证据必须为有界普通文件，不能经过链接".into());
    }
    let file = File::open(path)?;
    let opened = same_file::Handle::from_file(file.try_clone()?)?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if digest::linked(&after)
        || !after.is_file()
        || opened != same_file::Handle::from_path(path)?
        || bytes.len() > 16 * 1024 * 1024
    {
        return Err("身份证据在读取期间发生替换，保留现场".into());
    }
    Ok(Some(bytes))
}
pub(super) fn parse(bytes: &[u8]) -> Result<Value> {
    json::parse_fragment(std::str::from_utf8(bytes)?, "身份证据")
}
pub(super) fn new(path: &Path, value: &Value) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&encode(value)?)?;
    file.sync_all()?;
    Ok(())
}
pub(super) fn immutable(path: &Path, value: &Value) -> Result<()> {
    let bytes = encode(value)?;
    let pending = path.with_file_name(format!(
        "{}.pending",
        path.file_name()
            .and_then(|value| value.to_str())
            .ok_or("身份证据文件名无效")?
    ));
    for filename in [path, pending.as_path()] {
        if regular(filename)?.is_some_and(|current| current != bytes) {
            return Err("已有身份发布证据不同，拒绝覆盖".into());
        }
    }
    if regular(path)?.is_none() && regular(&pending)?.is_none() {
        new(&pending, value)?;
    }
    if regular(path)?.is_none() {
        match fs::hard_link(&pending, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    if regular(path)?.as_deref() != Some(bytes.as_slice()) {
        return Err("身份发布证据落盘后不同，保留现场".into());
    }
    if let Some(staged) = regular(&pending)? {
        if staged != bytes {
            return Err("身份发布暂存证据变化，保留现场".into());
        }
        let owned = same_file::Handle::from_path(&pending)?;
        if regular(&pending)?.as_deref() != Some(bytes.as_slice())
            || owned != same_file::Handle::from_path(&pending)?
        {
            return Err("身份暂存文件在清理前被替换".into());
        }
        fs::remove_file(pending)?;
    }
    Ok(())
}
pub(super) fn replace(path: &Path, value: &Value, expected: &[u8]) -> Result<Vec<u8>> {
    if regular(path)?.as_deref() != Some(expected) {
        return Err("身份账本已被替换，禁止覆盖未知前像".into());
    }
    json::write_replace(path, value)?;
    let bytes = encode(value)?;
    if regular(path)?.as_deref() != Some(bytes.as_slice()) {
        return Err("身份账本发布后不同，保留现场".into());
    }
    Ok(bytes)
}
