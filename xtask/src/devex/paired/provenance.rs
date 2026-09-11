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
    require_parent(backend_root, base_commit, contract)?;
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
    if contract == BaselineContract::LegacyStableReadinessB0V1 {
        adapter_paths = changed_paths(backend_root, base_commit, &adapter_commit)?;
        validate_stable_readiness_adapter(&patch, &patch_sha256, &adapter_paths)?;
        require_commit(
            frontend_root,
            "前端 B0",
            BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT,
            contract,
        )?;
        require_clean(frontend_root, "前端 B0", contract)?;
        frontend_commit = Some(BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT.to_owned());
    }

    Ok(Some(BaselineProvenance {
        base_commit: base_commit.to_owned(),
        adapter_commit,
        patch_sha256,
        frontend_commit,
        adapter_paths,
    }))
}

fn validate_stable_readiness_adapter(
    patch: &[u8],
    patch_sha256: &str,
    paths: &[String],
) -> Result<()> {
    if sha256(BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH)
        != BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
    {
        return Err("stable-readiness B0 内嵌适配资产摘要无效".into());
    }
    if patch_sha256 != BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
        || patch != BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH
    {
        return Err("stable-readiness B0 适配补丁摘要不匹配".into());
    }
    let expected = BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS;
    if paths.iter().map(String::as_str).ne(expected) {
        return Err(format!(
            "stable-readiness B0 适配越过工具层：预期 {expected:?}，实际 {paths:?}"
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
    let output = Command::new("git").args(args).current_dir(root).output()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_stable_readiness_adapter_has_registered_digest_and_scope() {
        let patch = BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH;
        assert_eq!(
            sha256(patch),
            BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
        );
        let patch = std::str::from_utf8(patch).unwrap();
        for path in BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS {
            assert!(patch.contains(&format!("diff --git a/{path} b/{path}")));
        }
        assert_eq!(
            patch.matches("diff --git ").count(),
            BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS.len()
        );
    }
}
