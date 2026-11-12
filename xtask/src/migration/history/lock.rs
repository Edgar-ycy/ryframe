use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use super::model::{
    LockDocument, LockedFile, lock_identity, safe_locked_path, sha256, valid_sha256,
};

pub(crate) fn parse_lock(
    root: &Path,
    source: &str,
    content: &[u8],
) -> (Option<LockDocument>, Vec<LockedFile>, Vec<String>) {
    let text = match std::str::from_utf8(content) {
        Ok(text) => text,
        Err(error) => {
            return (
                None,
                Vec::new(),
                vec![format!("无法解析迁移冻结清单 {source}: {error}")],
            );
        }
    };
    let value = match toml::from_str::<toml::Value>(text) {
        Ok(value) => value,
        Err(error) => {
            return (
                None,
                Vec::new(),
                vec![format!("无法解析迁移冻结清单 {source}: {error}")],
            );
        }
    };
    let document = LockDocument {
        frozen_at: value
            .get("frozen_at")
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
    };
    if value
        .get("format_version")
        .and_then(toml::Value::as_integer)
        != Some(1)
    {
        return (
            Some(document),
            Vec::new(),
            vec!["catalog/migrations.lock.toml format_version 必须为 1".to_owned()],
        );
    }
    let Some(files) = value.get("files").and_then(toml::Value::as_array) else {
        return (
            Some(document),
            Vec::new(),
            vec!["迁移冻结清单必须包含至少一个 [[files]]".to_owned()],
        );
    };
    if files.is_empty() {
        return (
            Some(document),
            Vec::new(),
            vec!["迁移冻结清单必须包含至少一个 [[files]]".to_owned()],
        );
    }
    let mut errors = Vec::new();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, value) in files.iter().enumerate() {
        let Some(entry) = value.as_table() else {
            errors.push(format!("files[{index}] 必须是对象"));
            continue;
        };
        let path_value = entry.get("path").and_then(toml::Value::as_str);
        let Some(path_value) = path_value else {
            errors.push("冻结文件 path 必须是字符串".to_owned());
            continue;
        };
        let (relative, path) = match safe_locked_path(root, path_value) {
            Ok(value) => value,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if !seen.insert(relative.clone()) {
            errors.push(format!("冻结文件重复：{relative}"));
            continue;
        }
        let expected = entry.get("sha256").and_then(toml::Value::as_str);
        let Some(expected) = expected.filter(|value| valid_sha256(value)) else {
            errors.push(format!("{relative} 的 sha256 必须是 64 位小写十六进制"));
            continue;
        };
        let (expected_storage, expected_target) = match lock_identity(&relative) {
            Ok(identity) => identity,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let storage = entry.get("storage").and_then(toml::Value::as_str);
        if storage != Some(expected_storage) {
            errors.push(format!(
                "{relative} 的 storage 必须是从路径推导的 {expected_storage:?}"
            ));
            continue;
        }
        let target = entry.get("target").and_then(toml::Value::as_str);
        if target != Some(expected_target.as_str()) {
            errors.push(format!(
                "{relative} 的 target 必须是从路径推导的 {expected_target:?}"
            ));
            continue;
        }
        entries.push(LockedFile {
            relative,
            path,
            expected_sha256: expected.to_owned(),
            storage: expected_storage.to_owned(),
            target: expected_target,
        });
    }
    (Some(document), entries, errors)
}

pub(crate) fn load_lock(
    root: &Path,
    lock_path: &Path,
) -> (Option<LockDocument>, Vec<LockedFile>, Vec<String>) {
    match fs::read(lock_path) {
        Ok(content) => parse_lock(root, &lock_path.display().to_string(), &content),
        Err(error) => (
            None,
            Vec::new(),
            vec![format!(
                "无法读取迁移冻结清单 {}: {error}",
                lock_path.display()
            )],
        ),
    }
}

pub(crate) fn verify_lock(root: &Path, lock_path: &Path) -> Vec<String> {
    let (_document, entries, mut errors) = load_lock(root, lock_path);
    for entry in entries {
        if !entry.path.is_file() {
            errors.push(format!("冻结迁移被删除或改名：{}", entry.relative));
            continue;
        }
        match sha256(&entry.path) {
            Ok(actual) if actual == entry.expected_sha256 => {}
            Ok(_) => errors.push(format!(
                "冻结迁移被修改：{}\n  源码与锁文件不一致；0.x 使用显式 baseline --write，稳定版请追加 roll-forward 迁移",
                entry.relative
            )),
            Err(error) => errors.push(error),
        }
    }
    errors
}

pub(super) fn render_lock(
    document: &LockDocument,
    entries: &[LockedFile],
) -> Result<Vec<u8>, String> {
    let frozen_at = document
        .frozen_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "迁移冻结清单 frozen_at 必须是非空字符串".to_owned())?;
    let mut rendered = format!("format_version = 1\nfrozen_at = \"{frozen_at}\"\n");
    for entry in entries {
        rendered.push_str(&format!(
            "\n[[files]]\npath = \"{}\"\nsha256 = \"{}\"\nstorage = \"{}\"\ntarget = \"{}\"\n",
            entry.relative, entry.expected_sha256, entry.storage, entry.target
        ));
    }
    Ok(rendered.into_bytes())
}

pub(crate) fn write_lock_atomically(
    lock_path: &Path,
    expected: &[u8],
    content: &[u8],
) -> Result<(), String> {
    let filename = lock_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("迁移冻结清单文件名无效：{}", lock_path.display()))?;
    let staged = lock_path.with_file_name(format!(".{filename}.freeze-{}", std::process::id()));
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|error| format!("无法创建迁移冻结清单暂存文件：{error}"))?;
        output
            .write_all(content)
            .and_then(|_| output.sync_all())
            .map_err(|error| format!("无法写入迁移冻结清单暂存文件：{error}"))?;
        let observed =
            fs::read(lock_path).map_err(|error| format!("无法复核迁移冻结清单：{error}"))?;
        if observed != expected {
            return Err("写入前迁移冻结清单发生变化，拒绝覆盖".to_owned());
        }
        fs::rename(&staged, lock_path).map_err(|error| format!("无法原子替换迁移冻结清单：{error}"))
    })();
    let _ = fs::remove_file(&staged);
    result
}

pub(super) fn lock_path(root: &Path) -> PathBuf {
    root.join(super::model::LOCK_RELATIVE_PATH)
}
