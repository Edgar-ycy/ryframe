use std::{path::Path, process::Command};

use sha2::{Digest, Sha256};

use crate::Result;

use crate::devex::model::{BaselineContract, BaselineProvenance};

pub(super) fn collect(
    backend_root: &Path,
    frontend_root: &Path,
    contract: Option<BaselineContract>,
) -> Result<Option<BaselineProvenance>> {
    let Some(contract) = contract else {
        return Ok(None);
    };
    let base_commit = contract.base_commit();
    let adapter_commit = git_text(backend_root, &["rev-parse", "HEAD"])?;
    if let Some(expected) = contract.required_adapter_commit() {
        require_commit(backend_root, "后端适配", expected, contract)?;
    }
    if contract == BaselineContract::LegacyStableReadinessB0V1 {
        require_b0(backend_root)?;
    } else {
        require_parent(backend_root, base_commit, contract)?;
    }
    require_clean(backend_root, "后端适配", contract)?;

    let patch = git_output(
        backend_root,
        &[
            "diff",
            "--binary",
            "--full-index",
            base_commit,
            &adapter_commit,
            "--",
            ".",
        ],
    )?;
    let patch_sha256 = sha256(&patch);
    let mut frontend_commit = None;
    let mut adapter_paths = Vec::new();
    let mut adapter_tree = None;
    if contract == BaselineContract::LegacyStableReadinessB0V1 {
        adapter_paths = changed_paths(backend_root, base_commit, &adapter_commit)?;
        let tree = git_text(backend_root, &["rev-parse", "HEAD^{tree}"])?;
        validate_stable_readiness_adapter(&patch, &patch_sha256, &adapter_paths, &tree)?;
        adapter_tree = Some(tree);
        require_commit(
            frontend_root,
            "前端 B0",
            crate::recovery::b0::FRONTEND,
            contract,
        )?;
        require_clean(frontend_root, "前端 B0", contract)?;
        frontend_commit = Some(crate::recovery::b0::FRONTEND.to_owned());
        require_b0(backend_root)?;
        require_commit(
            backend_root,
            "后端适配",
            crate::recovery::b0::COMMIT,
            contract,
        )?;
        require_clean(backend_root, "后端适配", contract)?;
    }

    Ok(Some(BaselineProvenance {
        base_commit: base_commit.to_owned(),
        adapter_commit,
        patch_sha256,
        adapter_tree,
        frontend_commit,
        adapter_paths,
    }))
}

fn validate_stable_readiness_adapter(
    patch: &[u8],
    patch_sha256: &str,
    paths: &[String],
    tree: &str,
) -> Result<()> {
    crate::recovery::b0::validate_patch(crate::recovery::b0::PATCH_BYTES)?;
    if patch_sha256 != format!("sha256:{}", crate::recovery::b0::PATCH_SHA256)
        || patch != crate::recovery::b0::PATCH_BYTES
    {
        return Err("stable-readiness B0 适配补丁摘要不匹配".into());
    }
    let expected = crate::recovery::b0::PATHS;
    if paths
        .iter()
        .map(String::as_str)
        .ne(expected.iter().copied())
    {
        return Err(format!(
            "stable-readiness B0 适配超出登记路径：预期 {expected:?}，实际 {paths:?}"
        )
        .into());
    }
    if tree != crate::recovery::b0::TREE {
        return Err(format!(
            "stable-readiness B0 适配树不匹配：预期 {}，实际 {tree}",
            crate::recovery::b0::TREE
        )
        .into());
    }
    Ok(())
}

fn require_commit(
    root: &Path,
    label: &str,
    expected: &str,
    contract: BaselineContract,
) -> Result<()> {
    let head = git_text(root, &["rev-parse", "HEAD"])?;
    if head != expected {
        return Err(format!(
            "{} {label}必须是已登记提交 {expected}，实际为 {head}",
            contract.as_str()
        )
        .into());
    }
    Ok(())
}

fn require_b0(root: &Path) -> Result<()> {
    use crate::recovery::b0;
    for (commit, tree) in [(b0::BACKEND, b0::BASE_TREE), (b0::COMMIT, b0::TREE)] {
        if !git_text(
            root,
            &[
                "for-each-ref",
                "--format=%(refname)",
                &format!("refs/replace/{commit}"),
            ],
        )?
        .is_empty()
            || git_text(root, &["rev-parse", &format!("{commit}^{{tree}}")])? != tree
        {
            return Err("B0 产品或执行对象存在替换引用或树不匹配".into());
        }
    }
    git_output(
        root,
        &["merge-base", "--is-ancestor", b0::BACKEND, b0::COMMIT],
    )?;
    Ok(())
}

fn require_parent(root: &Path, expected: &str, contract: BaselineContract) -> Result<()> {
    let parent = git_text(root, &["rev-parse", "HEAD^"])?;
    if parent != expected {
        return Err(format!(
            "{} 适配提交不再直接基于 B0 提交 {expected}",
            contract.as_str()
        )
        .into());
    }
    Ok(())
}

fn require_clean(root: &Path, label: &str, contract: BaselineContract) -> Result<()> {
    let status = git_text(root, &["status", "--porcelain", "--untracked-files=all"])?;
    if !status.is_empty() {
        return Err(format!("{} {label} worktree 必须干净", contract.as_str()).into());
    }
    Ok(())
}

fn changed_paths(root: &Path, base_commit: &str, adapter_commit: &str) -> Result<Vec<String>> {
    let output = git_output(
        root,
        &[
            "diff",
            "--name-only",
            "-z",
            base_commit,
            adapter_commit,
            "--",
            ".",
        ],
    )?;
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(str::to_owned)
                .map_err(|_| "baseline adapter 路径不是 UTF-8".into())
        })
        .collect()
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git_output(root, args)?)?
        .trim()
        .to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command
        .args(args)
        .current_dir(root)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "无法读取 legacy baseline 证据：git {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}
