//! 源登记的六文件本地发布与唯一前缀恢复；授权和资源观察由上层负责。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{json, lifecycle::state, resources::common},
};

pub(crate) const FILES: [&str; 6] = [
    "producer-lineage.json",
    "source-storage.json",
    "producers.json",
    "source-request.json",
    "generation-verified.json",
    "source-registration.json",
];

#[derive(PartialEq, Eq)]
struct Entry {
    binding: Value,
    identity: (u64, u64),
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    directories: Vec<(u64, u64)>,
    files: BTreeMap<String, Entry>,
}

pub(crate) fn output(backend: &Path, run: &Path, state: &Value, current: u64) -> Result<PathBuf> {
    if current == 0 {
        return Err("source-register 当前阶段序号必须为正整数".into());
    }
    common::local_directory(backend, run, "source-register 运行目录")?;
    let parent = run.join("seed-runtime");
    let parent_identity = directory_identity(backend, &parent)?;
    let previous = previous_outputs(backend, run, state, current)?;
    if previous.len() > 1 {
        return Err("多个历史 source-register 目录可能含登记，不能猜测唯一来源".into());
    }
    let selected = if let Some(path) = previous.keys().next() {
        path.clone()
    } else {
        let path = parent.join(format!("attempt-{current:04}"));
        common::ensure_new_directory(backend, &path, "source-register 输出目录")?;
        path
    };
    let selected_before = snapshot(backend, &selected)?;
    if previous.is_empty() && !selected_before.files.is_empty() {
        return Err("source-register 新输出目录出现未知写入；保留现场，禁止自动重放".into());
    }
    if directory_identity(backend, &parent)? != parent_identity
        || previous_outputs(backend, run, state, current)? != previous
        || snapshot(backend, &selected)? != selected_before
    {
        return Err("source-register 输出选择期间历史目录变化；保留现场，禁止自动重放".into());
    }
    Ok(selected)
}

pub(crate) fn write_or_match(
    backend: &Path,
    output: &Path,
    name: &str,
    value: &Value,
) -> Result<Value> {
    let index = FILES
        .iter()
        .position(|candidate| *candidate == name)
        .ok_or("source-register 文件名不属于固定六项证据")?;
    let encoded = readable_encoding(value)?;
    let mut expected = snapshot(backend, output)?;
    let path = output.join(name);
    if index < expected.files.len() {
        if json::read(&path)? != *value {
            return Err("source-register 已有局部证据不同；保留原文件并显式核对".into());
        }
    } else {
        if index != expected.files.len() {
            return Err("source-register 不能越过连续前缀发布证据".into());
        }
        validate_local_test_path(&path, backend, LocalTestPathKind::OutputFile)?;
        json::write_new(&path, value)?;
        let created = entry(backend, &path)?;
        if fs::read(&path)? != encoded || json::read(&path)? != *value {
            return Err("source-register 新证据写入期间变化；保留现场，禁止自动重放".into());
        }
        expected.files.insert(name.to_owned(), created);
    }
    let after = snapshot(backend, output)?;
    if after != expected || entry(backend, &path)? != *after.files.get(name).unwrap() {
        return Err("source-register 文件或目录集合变化；保留现场，禁止自动重放".into());
    }
    Ok(after.files.get(name).unwrap().binding.clone())
}

fn readable_encoding(value: &Value) -> Result<Vec<u8>> {
    if !value.is_object() {
        return Err("source-register 证据必须为 JSON 对象".into());
    }
    let mut encoded = serde_json::to_string_pretty(value)?;
    encoded.push('\n');
    if encoded.len() as u64 > json::MAX_JSON_BYTES {
        return Err("source-register 完整编码超过 JSON reader 的 16 MiB 预算".into());
    }
    if json::parse_fragment(&encoded, "source-register 完整编码")? != *value {
        return Err("source-register 完整编码无法按原值读取".into());
    }
    Ok(encoded.into_bytes())
}

fn previous_outputs(
    backend: &Path,
    run: &Path,
    state: &Value,
    current: u64,
) -> Result<BTreeMap<PathBuf, Snapshot>> {
    let mut found = BTreeMap::new();
    for attempt in state::attempts(state)? {
        let number = attempt["number"]
            .as_u64()
            .filter(|number| *number > 0)
            .ok_or("source-register 历史阶段序号无效")?;
        if number >= current
            || attempt["stage"] != "seed-runtime"
            || attempt["mode"] != "source-register"
        {
            continue;
        }
        let path = run
            .join("seed-runtime")
            .join(format!("attempt-{number:04}"));
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                if found
                    .insert(path.clone(), snapshot(backend, &path)?)
                    .is_some()
                {
                    return Err("source-register 历史阶段重复引用同一目录".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(found)
}

fn snapshot(backend: &Path, output: &Path) -> Result<Snapshot> {
    let parent = output.parent().ok_or("source-register 输出缺少父目录")?;
    let run = parent.parent().ok_or("source-register 输出缺少运行目录")?;
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let number = name
        .strip_prefix("attempt-")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|number| *number > 0)
        .ok_or("source-register 输出必须是固定 attempt 目录")?;
    if parent.file_name().and_then(|name| name.to_str()) != Some("seed-runtime")
        || name != format!("attempt-{number:04}")
    {
        return Err("source-register 输出不属于对应 run 的 seed-runtime/attempt 目录".into());
    }
    let directories = [run, parent, output]
        .into_iter()
        .map(|path| directory_identity(backend, path))
        .collect::<Result<Vec<_>>>()?;
    let mut files = BTreeMap::new();
    for item in fs::read_dir(output)? {
        let item = item?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "证据文件名必须为 UTF-8")?;
        if !FILES.contains(&name.as_str()) {
            return Err("source-register 目录含未知证据；保留现场并显式核对".into());
        }
        if files.insert(name, entry(backend, &item.path())?).is_some() {
            return Err("source-register 枚举期间出现重复文件；保留现场".into());
        }
    }
    if FILES[..files.len()]
        .iter()
        .any(|name| !files.contains_key(*name))
    {
        return Err("source-register 局部证据不是可恢复的连续前缀".into());
    }
    let after = [run, parent, output]
        .into_iter()
        .map(|path| directory_identity(backend, path))
        .collect::<Result<Vec<_>>>()?;
    if after != directories {
        return Err("source-register 枚举期间目录身份变化；保留现场".into());
    }
    Ok(Snapshot { directories, files })
}

fn directory_identity(backend: &Path, path: &Path) -> Result<(u64, u64)> {
    common::local_directory(backend, path, "source-register 证据目录")?;
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}

fn entry(backend: &Path, path: &Path) -> Result<Entry> {
    common::common_file(backend, path, "source-register 证据")?;
    let identity = common::device_inode_for(path, &fs::symlink_metadata(path)?)?;
    let binding = common::binding(path)?;
    common::common_file(backend, path, "source-register 证据")?;
    if common::device_inode_for(path, &fs::symlink_metadata(path)?)? != identity
        || common::binding(path)? != binding
    {
        return Err("source-register 读取期间证据身份或字节变化；保留现场".into());
    }
    Ok(Entry { binding, identity })
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/publication.rs"]
mod tests;
