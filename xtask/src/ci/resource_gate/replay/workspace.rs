use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use super::{
    path::{canonical_or_original, canonical_path, same_path, validate_inside},
    sha256_text,
};

#[derive(Debug, Clone)]
pub(crate) struct RepositoryIdentity {
    pub(crate) root: PathBuf,
    pub(crate) common_dir: PathBuf,
    pub(crate) remote: Option<String>,
}

pub(crate) fn repository_identity(
    path: &Path,
    label: &str,
    marker: &str,
) -> Result<RepositoryIdentity, String> {
    let root = canonical_path(path)
        .map_err(|error| format!("{label} replay 仓库目录不存在：{}：{error}", path.display()))?;
    if !root.is_dir() {
        return Err(format!("{label} replay 仓库目录不存在：{}", root.display()));
    }
    let top_level = PathBuf::from(git_output(&root, &["rev-parse", "--show-toplevel"])?);
    let top_level = canonical_path(&top_level)
        .map_err(|error| format!("无法解析{label} Git 顶层目录：{error}"))?;
    if !same_path(&root, &top_level) {
        return Err(format!(
            "{label} --repository 必须指向 Git 仓库顶层：{}",
            top_level.display()
        ));
    }
    if !root.join(marker).is_file() {
        return Err(format!("{label} replay 仓库缺少 {marker}"));
    }
    let common = PathBuf::from(git_output(&root, &["rev-parse", "--git-common-dir"])?);
    let common_dir = if common.is_absolute() {
        common
    } else {
        root.join(common)
    };
    let common_dir = canonical_path(&common_dir)
        .map_err(|error| format!("无法解析{label} Git common-dir：{error}"))?;
    let remote = git_optional_output(&root, &["config", "--get", "remote.origin.url"])?
        .filter(|value| !value.is_empty())
        .map(|value| sha256_text(&value));
    Ok(RepositoryIdentity {
        root,
        common_dir,
        remote,
    })
}

pub(crate) fn validate_repository_pair(
    backend: &RepositoryIdentity,
    frontend: &RepositoryIdentity,
) -> Result<(), String> {
    if same_path(&backend.common_dir, &frontend.common_dir) {
        return Err("前后端 replay 必须使用 git common-dir 不同的独立仓库".to_owned());
    }
    Ok(())
}

pub(crate) fn verify_case_ranges(
    backend: &RepositoryIdentity,
    frontend: &RepositoryIdentity,
    case: &super::manifest::ReplayCase,
) -> Result<(), String> {
    verify_commit_range(
        &backend.root,
        &case.base,
        &case.head,
        &case.name,
        "后端",
        false,
    )?;
    verify_commit_range(
        &frontend.root,
        &case.frontend_base,
        &case.frontend_head,
        &case.name,
        "前端",
        true,
    )
}

fn verify_commit_range(
    repository: &Path,
    base: &str,
    head: &str,
    case_name: &str,
    label: &str,
    allow_unchanged: bool,
) -> Result<(), String> {
    if !allow_unchanged && base == head {
        return Err(format!("replay 案例 {case_name} 的{label}范围没有变化"));
    }
    for commit in [base, head] {
        let reference = format!("{commit}^{{commit}}");
        if !git_success(repository, &["cat-file", "-e", &reference])? {
            return Err(format!("{label} fixture commit 不存在：{commit}"));
        }
    }
    if !git_success(repository, &["merge-base", "--is-ancestor", base, head])? {
        return Err(format!(
            "replay 案例 {case_name} 的{label} base 不是 head 祖先"
        ));
    }
    Ok(())
}

pub(crate) fn changed_files(
    repository: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>, String> {
    if base == head {
        return Ok(Vec::new());
    }
    let output = git_output(
        repository,
        &["diff", "--name-only", "--no-renames", base, head],
    )?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(crate) fn changed_file_count(
    repository: &Path,
    base: &str,
    head: &str,
) -> Result<usize, String> {
    Ok(changed_files(repository, base, head)?.len())
}

pub(crate) fn git_output(repository: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("Git 命令无法启动：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git 命令失败（{}）：{}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_optional_output(repository: &Path, arguments: &[&str]) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("Git 命令无法启动：{error}"))?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    } else {
        Ok(None)
    }
}

fn git_success(repository: &Path, arguments: &[&str]) -> Result<bool, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("Git 命令无法启动：{error}"))?;
    Ok(output.status.success())
}

pub(crate) fn add_worktree(
    repository: &Path,
    worktree: &Path,
    commit: &str,
    label: &str,
) -> Result<(), String> {
    let output = Command::new("git")
        .args(["worktree", "add", "--quiet", "--detach"])
        .arg(worktree)
        .arg(commit)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("Git 命令无法启动：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "无法创建{label} replay worktree：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(crate) fn switch_worktree(worktree: &Path, commit: &str, label: &str) -> Result<(), String> {
    switch_worktree_with(worktree, commit, label, |arguments| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(worktree)
            .output()
            .map_err(|error| format!("git 无法启动：{error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    })
}

pub(crate) fn switch_worktree_with(
    worktree: &Path,
    commit: &str,
    label: &str,
    mut run: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<(), String> {
    let current = run(&["rev-parse", "HEAD"])?;
    if current.eq_ignore_ascii_case(commit) {
        return Ok(());
    }
    run(&["checkout", "--quiet", "--detach", commit]).map_err(|reason| {
        format!(
            "无法切换{label} replay worktree {}：{reason}",
            worktree.display()
        )
    })?;
    Ok(())
}

pub(crate) fn cleanup_worktree(
    repository: &Path,
    worktree: &Path,
    label: &str,
    allowed: &Path,
) -> Result<(), String> {
    cleanup_worktree_with(repository, worktree, label, allowed, |arguments| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .output()
            .map_err(|error| format!("git 无法启动：{error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    })
}

pub(crate) fn cleanup_worktree_with(
    repository: &Path,
    worktree: &Path,
    label: &str,
    allowed: &Path,
    run: impl FnMut(&[&str]) -> Result<(), String>,
) -> Result<(), String> {
    cleanup_worktree_with_attempts(
        repository,
        worktree,
        label,
        allowed,
        run,
        cleanup_attempts(),
    )
}

pub(crate) fn cleanup_worktree_with_attempts(
    repository: &Path,
    worktree: &Path,
    label: &str,
    allowed: &Path,
    mut run: impl FnMut(&[&str]) -> Result<(), String>,
    attempts: usize,
) -> Result<(), String> {
    let mut last_error = String::new();
    let worktree_text = worktree.to_string_lossy();
    for attempt in 0..attempts {
        match run(&["worktree", "remove", "--force", worktree_text.as_ref()]) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(100 * (1_u64 << attempt)));
        }
    }
    if let Err(directory_error) = remove_partial_worktree_directory(worktree, allowed) {
        return Err(format!(
            "无法回收{label} replay worktree {}：{last_error}；{directory_error}",
            worktree.display()
        ));
    }
    run(&["worktree", "prune"]).map_err(|detail| {
        format!(
            "无法 prune {label} replay worktree（{}）：{detail}",
            repository.display()
        )
    })
}

fn cleanup_attempts() -> usize {
    if cfg!(windows) { 5 } else { 1 }
}

pub(crate) fn remove_partial_worktree_directory(
    worktree: &Path,
    allowed: &Path,
) -> Result<(), String> {
    let resolved = canonical_or_original(worktree);
    let allowed = canonical_or_original(allowed);
    validate_inside(&resolved, &allowed, "replay worktree")
        .map_err(|_| format!("拒绝清理允许范围外目录：{}", resolved.display()))?;
    remove_tree_retry(&resolved, 5)
        .map_err(|error| format!("无法删除残留目录 {}：{error}", resolved.display()))
}

pub(crate) fn remove_isolated_tree(path: &Path, allowed: &Path) -> Vec<String> {
    let resolved = canonical_or_original(path);
    let allowed = canonical_or_original(allowed);
    if validate_inside(&resolved, &allowed, "replay 清理目录").is_err() {
        return vec![format!("拒绝清理允许范围外目录：{}", resolved.display())];
    }
    if !resolved.exists() {
        return Vec::new();
    }
    match remove_tree_retry(&resolved, 5) {
        Ok(()) => {
            if let Some(parent) = resolved.parent() {
                let _ = fs::remove_dir(parent);
            }
            Vec::new()
        }
        Err(error) => vec![format!(
            "无法清理 replay 隔离目录 {}：{error}",
            resolved.display()
        )],
    }
}

fn remove_tree_retry(path: &Path, attempts: usize) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..attempts {
        let result = fs::remove_dir_all(path).and_then(|()| {
            if path.exists() {
                Err(std::io::Error::other("删除调用返回成功后目录仍存在"))
            } else {
                Ok(())
            }
        });
        match result {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error.to_string(),
        }
        make_writable(path);
        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(100 * (1_u64 << attempt)));
        }
    }
    Err(last_error)
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            make_writable(&entry.path());
        }
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(not(windows))]
fn make_writable(_: &Path) {}

pub(crate) fn cleanup_created_worktrees(
    entries: &[(bool, PathBuf, PathBuf, &str)],
    allowed: &Path,
) -> Vec<String> {
    entries
        .iter()
        .filter(|(created, _, _, _)| *created)
        .filter_map(|(_, repository, worktree, label)| {
            cleanup_worktree(repository, worktree, label, allowed).err()
        })
        .collect()
}

pub(crate) fn fail_on_cleanup(
    primary: Option<&str>,
    label: &str,
    errors: &[String],
) -> Result<(), String> {
    if errors.is_empty() {
        return Ok(());
    }
    let message = format!("{label}失败：{}", errors.join("；"));
    match primary {
        Some(primary) => Err(format!("{primary}；{message}")),
        None => Err(message),
    }
}

pub(crate) fn prune_worktrees(repositories: &[&Path]) -> Vec<String> {
    repositories
        .iter()
        .filter_map(|repository| {
            let output = Command::new("git")
                .args(["worktree", "prune"])
                .current_dir(repository)
                .output();
            match output {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(format!(
                    "无法 prune {}：{}",
                    repository.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => Some(format!("无法 prune {}：{error}", repository.display())),
            }
        })
        .collect()
}
