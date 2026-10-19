use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{digest, resources::common};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) fn snapshot(backend: &Path, target: &Path, locked: bool) -> Result<Value> {
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    let mut pending = vec![target.to_path_buf()];
    let mut entries = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let name = relative(target, &path)?;
            let metadata = fs::symlink_metadata(&path)?;
            if digest::linked(&metadata) {
                return Err("fresh 目标阶段文件经过链接或 junction".into());
            }
            if locked && name == "initialize.lock" {
                if !metadata.is_dir() || fs::read_dir(&path)?.next().is_some() {
                    return Err("目标初始化目录锁包含未知写入或类型变化".into());
                }
                continue;
            }
            if metadata.is_dir() {
                entries.push(json!({"path": name, "type": "directory"}));
                pending.push(path);
            } else if metadata.is_file() {
                if common::link_count_for(&path, &metadata)? != 1 {
                    return Err("fresh 目标阶段文件存在额外硬链接".into());
                }
                let actual = if locked && name == "copy-target.guard" {
                    if metadata.len() != 1 {
                        return Err("目标已持有的控制互斥文件长度无效".into());
                    }
                    digest::FileDigest {
                        bytes: 1,
                        sha256: hex::encode(Sha256::digest([0])),
                    }
                } else {
                    digest::file(&path)?
                };
                entries.push(json!({"path": name, "type": "file", "bytes": actual.bytes, "sha256": actual.sha256}));
            } else {
                return Err("fresh 目标阶段包含未知文件类型".into());
            }
        }
    }
    entries.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(Value::Array(entries))
}

fn relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)?
        .to_str()
        .ok_or("目标文件路径必须为 UTF-8")?
        .replace(std::path::MAIN_SEPARATOR, "/");
    validate_relative(&relative)?;
    Ok(relative)
}

pub(crate) fn files(target: &Path, tree: &Value) -> Result<Vec<PathBuf>> {
    tree.as_array()
        .ok_or("目标文件树必须为列表")?
        .iter()
        .filter(|entry| entry["type"] == "file" && entry["path"] != "copy-target.guard")
        .map(|entry| {
            let name = entry["path"].as_str().ok_or("目标文件树路径无效")?;
            validate_relative(name)?;
            Ok(target.join(name.replace('/', std::path::MAIN_SEPARATOR_STR)))
        })
        .collect()
}

fn validate_relative(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', ':'])
        || value.starts_with('/')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        return Err("目标文件树包含非规范或越界路径".into());
    }
    Ok(())
}
