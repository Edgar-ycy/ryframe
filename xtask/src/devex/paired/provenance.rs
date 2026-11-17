use std::{path::Path, process::Command};

use sha2::{Digest, Sha256};

use crate::Result;

use crate::devex::{
    model::{BaselineContract, BaselineProvenance, DevexPairedOptions, PairedArm, SaveAdaptation},
    targeted_source::TargetedPair,
};

#[path = "provenance/frontend.rs"]
mod frontend;

pub(super) fn collect(
    roots: &super::PairedRoots,
    options: &DevexPairedOptions,
) -> Result<Option<BaselineProvenance>> {
    let Some(contract) = options.baseline_contract else {
        return Ok(None);
    };
    let (backend_root, frontend_root) = roots.arm(PairedArm::Baseline);
    let targeted_pair = options
        .targeted_source
        .as_deref()
        .map(|path| {
            let candidate = roots.arm(PairedArm::Candidate);
            crate::devex::execution::verify_cache(backend_root)?;
            crate::devex::execution::verify_cache(candidate.0)?;
            TargetedPair::load(path, (backend_root, frontend_root), candidate)
        })
        .transpose()?;
    let base_commit = contract.base_commit();
    let save_adaptation =
        contract.registered_save_adaptation(options.run.suite, &options.run.variant);
    let adapter_commit = match (&targeted_pair, &save_adaptation) {
        (Some(pair), None) => pair.b0.backend_base.clone(),
        (None, Some(_)) => BaselineContract::STABLE_READINESS_B0_ADAPTER_COMMIT.to_owned(),
        (None, None) => git_text(backend_root, &["rev-parse", "HEAD"])?,
        (Some(_), Some(_)) => return Err("定向门禁与保存测量不能使用同一后继来源".into()),
    };
    let execution_backend = targeted_pair
        .as_ref()
        .map(|pair| pair.b0.backend_head.as_str())
        .or_else(|| save_adaptation.as_ref().map(|save| save.commit.as_str()))
        .unwrap_or(&adapter_commit);
    if let Some(expected) = contract.required_adapter_commit()
        && expected != adapter_commit
    {
        return Err(format!(
            "{} 后端适配应为 {expected}，实际为 {adapter_commit}",
            contract.as_str()
        )
        .into());
    }
    require_commit(backend_root, "后端适配", execution_backend, contract)?;
    require_parent(backend_root, &adapter_commit, base_commit, contract)?;
    if let Some(save) = &save_adaptation {
        verify_save_adaptation(backend_root, &adapter_commit, save, contract)?;
    }
    if contract.is_stable_readiness_b0() {
        require_stable_readiness_trees(backend_root)?;
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
    let frontend_adaptation = contract.registered_frontend_adaptation();
    let mut adapter_paths = Vec::new();
    let mut adapter_tree = None;
    if contract.is_stable_readiness_b0() {
        adapter_paths = changed_paths(backend_root, base_commit, &adapter_commit)?;
        let tree = git_text(
            backend_root,
            &["rev-parse", &format!("{adapter_commit}^{{tree}}")],
        )?;
        validate_stable_readiness_adapter(&patch, &patch_sha256, &adapter_paths, &tree)?;
        adapter_tree = Some(tree);
        let original = BaselineContract::STABLE_READINESS_B0_FRONTEND_COMMIT;
        let adapter = frontend_adaptation
            .as_ref()
            .map_or(original, |value| value.commit.as_str());
        let execution = targeted_pair
            .as_ref()
            .map_or(adapter, |pair| pair.b0.frontend_head.as_str());
        require_commit(frontend_root, "前端 B0", execution, contract)?;
        require_clean(frontend_root, "前端 B0", contract)?;
        if let Some(adaptation) = &frontend_adaptation {
            frontend::verify(frontend_root, original, adaptation)?;
            require_commit(frontend_root, "前端 B0", execution, contract)?;
            require_clean(frontend_root, "前端 B0", contract)?;
        }
        frontend_commit = Some(original.to_owned());
        require_commit(backend_root, "后端适配", execution_backend, contract)?;
        require_parent(backend_root, &adapter_commit, base_commit, contract)?;
        require_stable_readiness_trees(backend_root)?;
        require_clean(backend_root, "后端适配", contract)?;
    }

    Ok(Some(BaselineProvenance {
        base_commit: base_commit.to_owned(),
        adapter_commit,
        patch_sha256,
        adapter_tree,
        frontend_commit,
        frontend_adaptation,
        save_adaptation,
        adapter_paths,
        targeted_pair,
    }))
}

fn verify_save_adaptation(
    root: &Path,
    parent: &str,
    save: &SaveAdaptation,
    contract: BaselineContract,
) -> Result<()> {
    require_parent(root, &save.commit, parent, contract)?;
    let tree = git_text(root, &["rev-parse", &format!("{}^{{tree}}", save.commit)])?;
    let patch = git_output(
        root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--unified=0",
            parent,
            &save.commit,
            "--",
            ".",
        ],
    )?;
    if has_replacement(root, &save.commit)?
        || tree != save.tree
        || sha256(&patch) != save.patch_sha256
        || patch != BaselineContract::B0_SAVE_CONFIG_PATCH
    {
        return Err("B0 保存测量适配的 Git 树或补丁与登记不一致".into());
    }
    Ok(())
}

fn validate_stable_readiness_adapter(
    patch: &[u8],
    patch_sha256: &str,
    paths: &[String],
    tree: &str,
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
            "stable-readiness B0 适配超出登记路径：预期 {expected:?}，实际 {paths:?}"
        )
        .into());
    }
    if tree != BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE {
        return Err(format!(
            "stable-readiness B0 适配树不匹配：预期 {}，实际 {tree}",
            BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE
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

fn require_stable_readiness_trees(root: &Path) -> Result<()> {
    for (commit, tree) in [
        (
            BaselineContract::STABLE_READINESS_B0_BASE_COMMIT,
            BaselineContract::STABLE_READINESS_B0_BASE_TREE,
        ),
        (
            BaselineContract::STABLE_READINESS_B0_ADAPTER_COMMIT,
            BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE,
        ),
    ] {
        if has_replacement(root, commit)?
            || git_text(root, &["rev-parse", &format!("{commit}^{{tree}}")])? != tree
        {
            return Err("B0 产品或执行对象存在替换引用或树不匹配".into());
        }
    }
    Ok(())
}

fn has_replacement(root: &Path, object: &str) -> Result<bool> {
    Ok(!git_text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/replace/{object}"),
        ],
    )?
    .is_empty())
}

fn require_parent(
    root: &Path,
    commit: &str,
    expected: &str,
    contract: BaselineContract,
) -> Result<()> {
    let parent = git_text(root, &["rev-parse", &format!("{commit}^")])?;
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
