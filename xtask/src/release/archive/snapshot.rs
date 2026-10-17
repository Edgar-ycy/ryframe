use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use crate::Result;

use super::super::support::{commit_shape, read_json, sha256, sha256_file};
use super::{clean_snapshot, validate_relative_member};

pub(crate) fn source_pair_receipt(
    backend: &Path,
    frontend: &Path,
    run_id: u64,
    attempt: u64,
) -> Result<Value> {
    if run_id == 0 || attempt == 0 {
        return Err("全栈源码组合缺少有效运行身份".into());
    }
    let backend = fs::canonicalize(backend)
        .map_err(|error| format!("无法解析后端工作树 {}：{error}", backend.display()))?;
    let frontend = fs::canonicalize(frontend)
        .map_err(|error| format!("无法解析前端工作树 {}：{error}", frontend.display()))?;
    if backend == frontend {
        return Err("全栈源码组合必须来自两个独立仓库".into());
    }
    for (name, root) in [("backend", &backend), ("frontend", &frontend)] {
        let actual = git_root(root)?;
        if actual != *root {
            return Err(format!("全栈 {name} 来源必须是实际 Git 工作树根目录").into());
        }
    }
    let backend_snapshot = worktree_snapshot(&backend)?;
    let frontend_snapshot = worktree_snapshot(&frontend)?;
    clean_snapshot(
        &backend_snapshot,
        "全栈 backend 来源",
        backend_snapshot
            .get("head")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    clean_snapshot(
        &frontend_snapshot,
        "全栈 frontend 来源",
        frontend_snapshot
            .get("head")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    if worktree_snapshot(&backend)? != backend_snapshot
        || worktree_snapshot(&frontend)? != frontend_snapshot
    {
        return Err("采集全栈源码组合期间源码发生变化".into());
    }
    Ok(json!({
        "format_version": 1,
        "backend_sha": backend_snapshot["head"],
        "frontend_sha": frontend_snapshot["head"],
        "run_id": run_id,
        "attempt": attempt,
        "sources": {
            "backend": backend_snapshot,
            "frontend": frontend_snapshot,
        },
    }))
}

pub(crate) fn verify_source_pair_receipt(
    path: &Path,
    backend: &Path,
    frontend: &Path,
    run_id: u64,
    attempt: u64,
) -> Result<Value> {
    let (recorded, _) = read_json(path, "全栈源码组合收据", 16 * 1024)?;
    let current = source_pair_receipt(backend, frontend, run_id, attempt)?;
    if recorded != current {
        return Err("全栈源码组合收据与当前干净源码或运行身份不匹配".into());
    }
    Ok(recorded)
}

fn worktree_snapshot(root: &Path) -> Result<Value> {
    let head = git_text(root, &["rev-parse", "HEAD"])?;
    if !commit_shape(Some(&head)) {
        return Err("源码 HEAD 不是有效 SHA".into());
    }
    let patch = git_bytes(root, &["diff", "--binary", "HEAD"])?;
    let untracked = git_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut paths = Vec::new();
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        paths.push(
            std::str::from_utf8(path)
                .map_err(|_| "源码清单路径不是 UTF-8")?
                .to_owned(),
        );
    }
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for relative in paths {
        let path = checked_source_file(root, &relative)?;
        files.push(json!({
            "path": relative,
            "sha256": sha256_file(&path, "未跟踪源码文件")?,
        }));
    }
    Ok(json!({
        "head": head,
        "patch_sha256": sha256(&patch),
        "files": files,
    }))
}

fn checked_source_file(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_member(relative)?;
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法读取源码路径 {}：{error}", path.display()))?;
        if metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata) {
            return Err("源码路径不能包含符号链接或 junction".into());
        }
    }
    if !path.is_file() {
        return Err("源码清单只能绑定普通文件".into());
    }
    let resolved = fs::canonicalize(&path)
        .map_err(|error| format!("无法解析源码路径 {}：{error}", path.display()))?;
    if !resolved.starts_with(root) {
        return Err("源码文件越界".into());
    }
    Ok(path)
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn git_root(root: &Path) -> Result<PathBuf> {
    let text = git_text(root, &["rev-parse", "--show-toplevel"])?;
    fs::canonicalize(&text)
        .map_err(|error| format!("无法解析 Git 工作树根目录 {text}：{error}").into())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let bytes = git_bytes(root, args)?;
    String::from_utf8(bytes)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "Git 输出不是 UTF-8".into())
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("无法启动 git {}：{error}", args.join(" ")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}
