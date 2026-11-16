use std::{fs, path::Path, process::Command};

use sha2::{Digest, Sha256};

use crate::Result;

use super::{
    format_digest,
    schema::{RunMetadata, SourceFingerprints, SourceState},
};

pub(in crate::devex) fn source_state(root: &Path) -> Result<SourceState> {
    let commit = git_text(root, &["rev-parse", "HEAD"])
        .ok_or_else(|| format!("无法读取源码提交：{}", root.display()))?;
    let status = git_text(root, &["status", "--porcelain", "--untracked-files=all"])
        .ok_or_else(|| format!("无法读取源码状态：{}", root.display()))?;
    Ok(SourceState {
        commit: Some(commit.clone()),
        dirty: Some(!status.is_empty()),
        worktree_fingerprint: worktree_fingerprint(root, &commit)?,
    })
}

pub(in crate::devex) fn collect_source_fingerprints(
    backend_root: &Path,
    frontend_root: Option<&Path>,
) -> Result<SourceFingerprints> {
    Ok(SourceFingerprints {
        backend: source_state(backend_root)?.worktree_fingerprint,
        frontend: frontend_root
            .map(source_state)
            .transpose()?
            .map(|state| state.worktree_fingerprint),
        runner_frontend: None,
    })
}

pub(in crate::devex) fn collect_runtime_source_fingerprints(
    backend_root: &Path,
    frontend_root: &Path,
    runner_frontend_root: Option<&Path>,
) -> Result<SourceFingerprints> {
    let mut fingerprints = collect_source_fingerprints(backend_root, Some(frontend_root))?;
    fingerprints.runner_frontend = runner_frontend_root
        .map(source_state)
        .transpose()?
        .map(|state| state.worktree_fingerprint);
    Ok(fingerprints)
}

impl RunMetadata {
    pub(in crate::devex) fn source_fingerprints(&self) -> SourceFingerprints {
        SourceFingerprints {
            backend: self.backend.worktree_fingerprint.clone(),
            frontend: self
                .frontend
                .as_ref()
                .map(|state| state.worktree_fingerprint.clone()),
            runner_frontend: self
                .runner_frontend
                .as_ref()
                .map(|state| state.worktree_fingerprint.clone()),
        }
    }
}

fn worktree_fingerprint(root: &Path, commit: &str) -> Result<String> {
    let tracked = git_output(
        root,
        &["diff", "--binary", "--no-ext-diff", "HEAD", "--", "."],
    )?;
    let untracked = git_output(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut digest = Sha256::new();
    update_digest(&mut digest, commit.as_bytes());
    update_digest(&mut digest, &tracked);
    for raw_path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(raw_path)
            .map_err(|_| "未跟踪文件路径不是 UTF-8，无法建立源码指纹")?;
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(format!("Git 返回越界的未跟踪路径：{relative}").into());
        }
        update_digest(&mut digest, raw_path);
        update_digest(&mut digest, &fs::read(root.join(relative_path))?);
    }
    Ok(format_digest(digest.finalize()))
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
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

fn update_digest(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
