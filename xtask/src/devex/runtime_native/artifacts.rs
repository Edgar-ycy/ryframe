use super::failure::{Failure, NativeResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

pub(crate) fn parents(path: &Path) -> NativeResult<()> {
    for ancestor in path.ancestors() {
        if let Ok(metadata) = fs::symlink_metadata(ancestor)
            && linked(&metadata)
        {
            return Err(Failure::new("artifact_link"));
        }
    }
    Ok(())
}

pub(crate) fn directory(path: &Path) -> NativeResult<()> {
    parents(path)?;
    fs::create_dir_all(path).map_err(|_| Failure::new("artifact_directory"))?;
    parents(path)
}

pub(crate) fn read(path: &Path, limit: u64) -> NativeResult<Vec<u8>> {
    parents(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| Failure::new("artifact_read"))?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() > limit {
        return Err(Failure::new("artifact_size_or_type"));
    }
    let file = File::open(path).map_err(|_| Failure::new("artifact_read"))?;
    let handle = same_file::Handle::from_file(
        file.try_clone()
            .map_err(|_| Failure::new("artifact_read"))?,
    )
    .map_err(|_| Failure::new("artifact_read"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::new("artifact_read"))?;
    if bytes.len() as u64 > limit {
        return Err(Failure::new("artifact_size_or_type"));
    }
    let after = fs::symlink_metadata(path).map_err(|_| Failure::new("artifact_changed"))?;
    let current =
        same_file::Handle::from_path(path).map_err(|_| Failure::new("artifact_changed"))?;
    if current != handle
        || linked(&after)
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        return Err(Failure::new("artifact_changed"));
    }
    Ok(bytes)
}

pub(crate) fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 同目录临时文件完整落盘后以不覆盖方式发布；已有证据始终保留。
pub(crate) fn write(path: &Path, value: &impl Serialize) -> NativeResult<()> {
    let parent = path.parent().ok_or_else(|| Failure::new("artifact_path"))?;
    directory(parent)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| Failure::new("artifact_json"))?;
    bytes.push(b'\n');
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| Failure::new("artifact_create"))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| Failure::new("artifact_write"))?;
    parents(path)?;
    temporary
        .persist_noclobber(path)
        .map_err(|_| Failure::new("artifact_publication_unknown"))?;
    Ok(())
}

/// 周期收据追加失败即失败关闭；不重试未知追加。
pub(crate) fn append(path: &Path, rows: &[serde_json::Value]) -> NativeResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    parents(path)?;
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|_| Failure::new("artifact_append"))?;
        if linked(&metadata) || !metadata.is_file() {
            return Err(Failure::new("artifact_link"));
        }
    }
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).map_err(|_| Failure::new("artifact_json"))?;
        bytes.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| Failure::new("artifact_append"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Failure::new("artifact_append_unknown"))
}
