use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

/// 返回后端 Cargo Workspace 根目录。
pub(crate) fn root_dir() -> PathBuf {
    if let Ok(target) = std::env::var("RYFRAME_DEVEX_TARGET_ROOT") {
        let target = PathBuf::from(target);
        if let Some(session) = target.parent() {
            let candidate = session.join("worktree").join("b");
            if candidate.join("Cargo.toml").is_file() && candidate.join("xtask").is_dir() {
                return candidate;
            }
        }
    }
    if let Ok(explicit) = std::env::var("RYFRAME_WORKSPACE_ROOT") {
        let root = PathBuf::from(explicit);
        if root.join("Cargo.toml").is_file() && root.join("xtask").is_dir() {
            return root;
        }
    }
    if let Ok(current) = std::env::current_dir()
        && let Some(root) = current.ancestors().find(|candidate| {
            candidate.join("Cargo.toml").is_file() && candidate.join("xtask").is_dir()
        })
    {
        return root.to_path_buf();
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 必须位于 Cargo Workspace 根目录下")
        .to_path_buf()
}

/// 返回前后端并列检出时的默认前端目录。
pub(crate) fn default_frontend_dir() -> PathBuf {
    root_dir()
        .parent()
        .expect("后端 Workspace 必须具有父目录")
        .join("ryframe-vue3")
}

/// 删除显式隔离根目录内的单个子目录，并拒绝越界或根目录目标。
pub(crate) fn remove_isolated_directory(root: &Path, target: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("无法规范化隔离目录 {}：{error}", root.display()))?;
    let target = target
        .canonicalize()
        .map_err(|error| format!("无法规范化待删除目录 {}：{error}", target.display()))?;
    if target == root || !target.starts_with(&root) {
        return Err(format!(
            "拒绝删除隔离边界外的目录：root={}，target={}",
            root.display(),
            target.display()
        )
        .into());
    }
    fs::remove_dir_all(target)?;
    Ok(())
}
