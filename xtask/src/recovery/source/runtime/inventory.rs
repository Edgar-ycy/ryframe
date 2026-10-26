//! 来源验收授权前后的完整文件清单；动态采集记录只能按既有类别和身份增长。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, resources::common},
};

#[path = "inventory/dynamic.rs"]
mod dynamic;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/source/runtime/inventory/tests.rs"]
mod tests;

const PRODUCER_FILES: &[&str] = &[
    "source-producer-intent.json",
    "source-producer.json",
    "source-producer-ready.json",
    "source-producer.stdout.json",
    "source-producer.stderr.log",
    "source-producer-completion.json",
    "source-producer-configuration.json",
];
const DIRECTORIES: &[&str] = &["before", "after", "audit", "source-tools", "recovery-audit"];
const POST_FILES: &[&str] = &[
    "cache-cleanup-attempt.json",
    "cache-cleanup-execution.json",
    "failed.json",
    "cache-cleanup.json",
    "source-runtime.json",
    "source-verification-recovery-intent.json",
    "source-verification-recovery.json",
    "audit/login-after-old.tsv",
    "audit/login-after-new.tsv",
    "audit/login-audit.json",
];

pub(super) fn request(args: &Value, verify: bool) -> Result<Value> {
    let fields = if verify {
        &["backend", "directory", "inputs"][..]
    } else {
        &["backend", "directory"][..]
    };
    documents::exact(args, fields, "来源证据文件参数")?;
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let directory = Path::new(documents::text(&args["directory"], "来源证据目录")?);
    validate_directory(directory, backend)?;
    if verify {
        verify_inputs(directory, &args["inputs"])
    } else {
        Ok(Value::Array(manifest(directory)?))
    }
}

pub(super) fn files_request(args: &Value) -> Result<Value> {
    documents::exact(args, &["backend", "directory"], "来源生产者文件参数")?;
    let backend = Path::new(documents::text(&args["backend"], "来源后端")?);
    let directory = Path::new(documents::text(&args["directory"], "来源证据目录")?);
    validate_directory(directory, backend)?;
    Ok(Value::Array(evidence_files(directory)?))
}

fn validate_directory(directory: &Path, backend: &Path) -> Result<()> {
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "来源证据目录必须位于后端忽略目录，且不能经过链接或重解析点".into())
        .map(|_| ())
}

fn descriptor(path: &Path) -> Result<Value> {
    common::binding(path).map_err(|_| "来源证据普通文件读取失败，保留原始文件后核对".into())
}

fn walk(root: &Path, directory: &Path, rows: &mut Vec<Value>) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || digest::linked(&metadata) {
        return Err("来源验收证据目录不是普通目录".into());
    }
    let entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.is_empty() {
        return Err("来源验收证据包含空目录".into());
    }
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if digest::linked(&metadata) {
            return Err("来源验收证据包含链接或重解析点".into());
        }
        if metadata.is_dir() {
            walk(root, &path, rows)?;
        } else if metadata.is_file() {
            let mut descriptor = descriptor(&path)?;
            descriptor["path"] = json!(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            rows.push(descriptor);
        } else {
            return Err("来源验收证据包含非普通文件".into());
        }
    }
    Ok(())
}

pub(super) fn manifest(root: &Path) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    walk(root, root, &mut rows)?;
    rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(rows)
}

fn evidence_files(directory: &Path) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if digest::linked(&metadata) {
            return Err("来源验收中间证据包含链接或重解析点".into());
        }
        if metadata.is_dir() {
            if !DIRECTORIES
                .iter()
                .any(|name| path.file_name() == Some(name.as_ref()))
            {
                return Err("来源验收中间证据包含未知目录".into());
            }
            // 仅阶段根允许为空；嵌套空目录仍由同一 manifest 拒绝。
            if fs::read_dir(&path)?.next().is_some() {
                walk(directory, &path, &mut rows)?;
            }
        } else if metadata.is_file() {
            let mut descriptor = descriptor(&path)?;
            descriptor["path"] = json!(
                path.file_name()
                    .ok_or("来源文件名缺失")?
                    .to_str()
                    .ok_or("来源文件名不是UTF-8")?
            );
            rows.push(descriptor);
        } else {
            return Err("来源验收中间证据包含非普通文件".into());
        }
    }
    rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(rows)
}

fn verify_inputs(directory: &Path, inputs: &Value) -> Result<Value> {
    let inputs = documents::array(inputs, "来源生产者授权前证据清单")?;
    let mut previous = "";
    for row in inputs {
        documents::exact(row, &["path", "bytes", "sha256"], "来源生产者授权前证据")?;
        let path = documents::text(&row["path"], "来源证据相对路径")?;
        if path <= previous
            || !["before/", "audit/", "source-tools/"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
        {
            return Err("来源生产者授权前证据路径不属于固定采集阶段".into());
        }
        previous = path;
    }
    let observed = evidence_files(directory)?;
    let by_path = observed
        .iter()
        .map(|row| Ok((documents::text(&row["path"], "来源证据路径")?, row)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for row in inputs {
        if by_path.get(documents::text(&row["path"], "来源证据路径")?) != Some(&row) {
            return Err("来源生产者授权前完整证据已变化".into());
        }
    }
    let mut allowed: BTreeSet<String> = PRODUCER_FILES
        .iter()
        .chain(POST_FILES)
        .map(|name| (*name).into())
        .collect();
    for row in inputs {
        let path = documents::text(&row["path"], "来源证据路径")?;
        allowed.insert(path.into());
        if let Some(suffix) = path.strip_prefix("before/") {
            allowed.insert(format!("after/{suffix}"));
        }
    }
    dynamic::query_paths(directory, inputs, &observed, &mut allowed)?;
    dynamic::image_paths(directory, inputs, &observed, &mut allowed)?;
    dynamic::database_paths(directory, inputs, &observed, &mut allowed)?;
    if by_path.keys().any(|path| !allowed.contains(*path)) {
        return Err("来源验收中间证据包含未登记文件".into());
    }
    Ok(Value::Array(observed))
}

fn selected<'a>(rows: &'a [Value], pattern: &str) -> Result<Vec<&'a str>> {
    let pattern = regex::Regex::new(pattern)?;
    rows.iter()
        .filter_map(|row| row["path"].as_str())
        .filter(|path| pattern.is_match(path))
        .map(Ok)
        .collect()
}

fn relative(directory: &Path, name: &str) -> PathBuf {
    directory.join(name)
}
