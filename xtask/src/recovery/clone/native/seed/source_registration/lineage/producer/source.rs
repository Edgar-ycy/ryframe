use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as json_io, resources::common},
        fixture_control::portable_path,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn unlinked(path: &Path) -> Result<()> {
    if digest::linked_path(path)?.iter().any(|v| *v) {
        return Err("生产者路径不能经过链接".into());
    }
    Ok(())
}
pub(super) fn directory(path: &Path, parent: &Path) -> Result<PathBuf> {
    unlinked(path)?;
    if path == parent || !path.starts_with(parent) || !path.is_dir() {
        return Err("生产者目录必须在明确范围内".into());
    }
    Ok(portable_path(&fs::canonicalize(path)?))
}
pub(super) fn regular(
    path: &Path,
    parent: &Path,
    minimum: u64,
    maximum: Option<u64>,
) -> Result<PathBuf> {
    unlinked(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !path.starts_with(parent)
        || !metadata.is_file()
        || metadata.len() < minimum
        || maximum.is_some_and(|max| metadata.len() > max)
    {
        return Err("生产者证据必须为明确范围内的普通文件".into());
    }
    Ok(portable_path(&fs::canonicalize(path)?))
}
pub(super) fn sha(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| "生产者摘要无效".into())
}
pub(super) fn binding_path(value: &Value) -> Result<PathBuf> {
    json_io::exact(value, &["path", "bytes", "sha256"], "生产者文件绑定")?;
    Ok(PathBuf::from(json_io::text(&value["path"], "生产者路径")?))
}
pub(super) fn bound(path: &Path, parent: &Path, expected: Option<&Value>) -> Result<Value> {
    let path = regular(path, parent, 1, Some(json_io::MAX_JSON_BYTES))?;
    let actual = common::binding(&path)?;
    if let Some(expected) = expected {
        binding_path(expected)?;
        sha(&expected["sha256"])?;
        if *expected != actual {
            return Err("生产者文件绑定已变化".into());
        }
    }
    Ok(actual)
}
pub(super) fn tool(value: &Value) -> Result<PathBuf> {
    json_io::exact(value, &["path", "sha256"], "生产者工具")?;
    let path = PathBuf::from(json_io::text(&value["path"], "生产者工具路径")?);
    unlinked(&path)?;
    if !path.is_file() || digest::file(&path)?.sha256 != sha(&value["sha256"])? {
        return Err("Node 工具路径或摘要已变化".into());
    }
    Ok(portable_path(&fs::canonicalize(path)?))
}
pub(super) fn identity(value: &Value, executable: Option<&Path>) -> Result<Value> {
    json_io::exact(value, &["pid", "started", "executable"], "历史进程身份")?;
    let started = json_io::text(&value["started"], "历史创建代次")?;
    let path = PathBuf::from(json_io::text(&value["executable"], "历史可执行文件")?);
    unlinked(&path)?;
    if value["pid"].as_u64().is_none_or(|n| n <= 1)
        || started.is_empty()
        || started.len() > 32
        || started.starts_with('0')
        || !started.bytes().all(|b| b.is_ascii_digit())
        || !path.is_file()
    {
        return Err("历史进程缺少完整创建身份".into());
    }
    let path = portable_path(&fs::canonicalize(path)?);
    if let Some(expected) = executable
        && path != portable_path(&fs::canonicalize(expected)?)
    {
        return Err("历史可执行文件与登记工具不同".into());
    }
    Ok(json!({"pid":value["pid"],"started":started,"executable":path}))
}
pub(super) fn snapshot(
    attempt: &Value,
    backend: &Path,
    script: &Path,
    descriptor: &Value,
) -> Result<()> {
    sha(&descriptor["sha256"])?;
    let sources = &attempt["sources"];
    json_io::exact(
        sources,
        &["snapshot", "worktree_fingerprint", "fingerprints"],
        "attempt 来源",
    )?;
    let snapshot = &sources["snapshot"];
    json_io::exact(
        snapshot,
        &["head", "patch_sha256", "files", "clean"],
        "历史来源快照",
    )?;
    let head = snapshot["head"].as_str().ok_or("历史 HEAD 无效")?;
    let fingerprint = sources["worktree_fingerprint"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .ok_or("历史工作树指纹无效")?;
    sha(&json!(fingerprint))?;
    if head.len() != 40
        || !head
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !snapshot["clean"].is_boolean()
    {
        return Err("Node attempt 来源快照结构无效".into());
    }
    sha(&snapshot["patch_sha256"])?;
    let fingerprints = json_io::exact(
        &sources["fingerprints"],
        &["product", "test_tools", "support"],
        "分域指纹",
    )?;
    for value in fingerprints.values() {
        json_io::exact(value, &["sha256", "files"], "分域指纹")?;
        sha(&value["sha256"])?;
        if value["files"].as_u64().is_none() {
            return Err("分域指纹文件数无效".into());
        }
    }
    let relative = script
        .strip_prefix(backend)?
        .to_str()
        .ok_or("脚本路径必须为 UTF-8")?
        .replace('\\', "/");
    let files = snapshot["files"].as_array().ok_or("来源清单必须为数组")?;
    let mut previous = "";
    let mut found = false;
    for item in files {
        json_io::exact(item, &["path", "sha256"], "历史来源项")?;
        let path = item["path"].as_str().ok_or("历史来源路径无效")?;
        // PurePosixPath 的规范形式：保留反斜杠普通字符，拒绝绝对路径、跳转及折叠写法。
        if path <= previous
            || path.starts_with('/')
            || (path != "."
                && path
                    .split('/')
                    .any(|p| p.is_empty() || p == "." || p == ".."))
        {
            return Err("历史来源清单未排序、重复或越界".into());
        }
        previous = path;
        sha(&item["sha256"])?;
        if path == relative {
            found = true;
            if item["sha256"] != descriptor["sha256"] {
                return Err("历史脚本与来源快照不一致".into());
            }
        }
    }
    if !found && common::binding(script)? != *descriptor {
        return Err("历史脚本既不属于快照，也不同于当前固定文件".into());
    }
    Ok(())
}

pub(super) fn script(
    attempt: &Value,
    backend: &Path,
    kind: &str,
    descriptor: &Value,
) -> Result<()> {
    let path = binding_path(descriptor)?;
    if path != crate::recovery::clone::native::post_process::command::source(backend, kind)?
        || descriptor["bytes"].as_u64().is_none_or(|n| n == 0)
    {
        return Err("Node 历史脚本绑定无效".into());
    }
    regular(&path, backend, 1, Some(json_io::MAX_JSON_BYTES))?;
    snapshot(attempt, backend, &path, descriptor)
}
