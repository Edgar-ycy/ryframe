use std::path::Path;

use crate::{Result, devex::model::FrontendAdaptation};

use super::{changed_paths, git_output, git_text, has_replacement, sha256};

const ADAPTER_PATHS: [&str; 3] = [".node-version", "package.json", "pnpm-lock.yaml"];

pub(super) fn verify(root: &Path, original: &str, evidence: &FrontendAdaptation) -> Result<()> {
    for (commit, tree) in [
        (original, evidence.original_tree.as_str()),
        (evidence.commit.as_str(), evidence.tree.as_str()),
    ] {
        if has_replacement(root, commit)?
            || has_replacement(root, tree)?
            || git_text(root, &["rev-parse", &format!("{commit}^{{tree}}")])? != tree
        {
            return Err("DevEx B0 前端原树或测量树存在替换引用或身份不匹配".into());
        }
    }
    let commit = git_output(root, &["cat-file", "commit", &evidence.commit])?;
    let parents = commit
        .split(|byte| *byte == b'\n')
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.strip_prefix(b"parent "));
    if parents.ne([original.as_bytes()]) {
        return Err("DevEx B0 前端测量提交必须直接且仅基于登记原提交".into());
    }
    let paths = changed_paths(root, original, &evidence.commit)?;
    let patch = git_output(
        root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            original,
            &evidence.commit,
            "--",
            ".",
        ],
    )?;
    if paths.iter().map(String::as_str).ne(ADAPTER_PATHS) || sha256(&patch) != evidence.patch_sha256
    {
        return Err("DevEx B0 前端工具链适配的路径或补丁摘要不匹配".into());
    }
    verify_toolchain_files(root, original, evidence)
}

fn verify_toolchain_files(
    root: &Path,
    original: &str,
    evidence: &FrontendAdaptation,
) -> Result<()> {
    let commit = &evidence.commit;
    let node = blob(root, commit, ".node-version")?;
    let package = blob(root, commit, "package.json")?;
    if node.as_slice() != b"24.21.0\n"
        || sha256(&package)
            != "sha256:bcb75a4a31dd86fc0ee681bf5480f7adffb445e744d41fa4a4be7c5313e83f30"
        || serde_json::from_slice::<serde_json::Value>(&package)?["packageManager"] != "pnpm@12.8.1"
    {
        return Err("DevEx B0 前端测量的 Node 或 pnpm 固定文件不匹配".into());
    }
    let application = blob(root, original, "pnpm-lock.yaml")?;
    let combined = blob(root, commit, "pnpm-lock.yaml")?;
    // 第二份文档前的分隔符不属于任何一份已登记锁文件。
    let manager = combined
        .strip_suffix(application.as_slice())
        .and_then(|prefix| prefix.strip_suffix(b"---\n"))
        .ok_or("DevEx B0 前端测量锁的应用文档不是原始字节")?;
    if sha256(&application) != evidence.application_lock_sha256
        || sha256(manager) != evidence.manager_lock_sha256
    {
        return Err("DevEx B0 前端测量锁的应用或包管理器文档摘要不匹配".into());
    }
    Ok(())
}

fn blob(root: &Path, commit: &str, path: &str) -> Result<Vec<u8>> {
    git_output(root, &["cat-file", "blob", &format!("{commit}:{path}")])
}
