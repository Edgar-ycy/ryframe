use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileDigest {
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

pub(crate) fn plan_hash(value: &Value) -> Result<String> {
    let encoded = serde_json::to_vec(value)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub(crate) fn file(path: &Path) -> Result<FileDigest> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法读取复制证据 {}：{error}", path.display()))?;
    if !metadata.file_type().is_file() || linked(&metadata) {
        return Err(format!("复制证据必须是普通文件：{}", path.display()).into());
    }
    let stream = File::open(path)
        .map_err(|error| format!("无法打开复制证据 {}：{error}", path.display()))?;
    let actual = stream.metadata()?;
    if !actual.is_file() || linked(&actual) {
        return Err(format!("复制证据打开后不是普通文件：{}", path.display()).into());
    }
    let mut reader = BufReader::new(stream);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes = bytes.checked_add(read as u64).ok_or("复制证据字节数溢出")?;
    }
    Ok(FileDigest {
        bytes,
        sha256: hex::encode(digest.finalize()),
    })
}

pub(crate) fn canonical_binding(digest: &FileDigest) -> String {
    format!("{}:{}", digest.bytes, digest.sha256)
}

pub(crate) fn safe_file(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(|character| (character as u32) < 32)
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("备份文件路径越界或格式无效".into());
    }
    let candidate = root.join(relative);
    validate_components(root, &candidate)?;
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| format!("复制证据缺失或无法核验：{error}"))?;
    if !metadata.is_file() || linked(&metadata) {
        return Err("复制证据必须是普通文件且不能经过链接".into());
    }
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析复制证据根目录：{error}"))?;
    let canonical = fs::canonicalize(&candidate)
        .map_err(|error| format!("无法解析复制证据 {}：{error}", candidate.display()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("复制证据解析后越过已登记根目录".into());
    }
    Ok(candidate)
}

fn validate_components(root: &Path, candidate: &Path) -> Result<()> {
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| "复制证据必须位于已登记根目录")?;
    if relative
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("复制证据包含路径跳转".into());
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("复制证据路径不存在或无法核验：{error}"))?;
        if linked(&metadata) {
            return Err(format!("复制证据路径经过链接：{}", current.display()).into());
        }
    }
    Ok(())
}

pub(crate) fn path_text(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}

#[cfg(windows)]
pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
