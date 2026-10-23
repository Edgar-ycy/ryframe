use super::{
    ReplayContext, ReplayStep, ReplayWorkspace, cleanup_created_worktrees, prune_worktrees,
    remove_isolated_tree, stop_sccache,
};
use std::{fs, path::Path};

pub(super) fn prepare_workspace(
    work_root: &Path,
    session_name: &str,
) -> ReplayStep<ReplayWorkspace> {
    let session = work_root.join(session_name);
    if session.exists() {
        return Err(format!("replay session 已存在：{}", session.display()));
    }
    fs::create_dir(&session)
        .map_err(|error| format!("无法创建 replay session {}：{error}", session.display()))?;
    let cache = work_root.join("cache").join(session_name);
    if cache.exists() {
        let _ = fs::remove_dir(&session);
        return Err(format!(
            "replay sccache 目录已存在，拒绝复用：{}",
            cache.display()
        ));
    }
    if let Err(error) = fs::create_dir_all(&cache) {
        let _ = fs::remove_dir(&session);
        return Err(format!(
            "无法创建 replay sccache 目录 {}：{error}",
            cache.display()
        ));
    }
    let shared_target = session.join("target");
    let shared_worktree = session.join("worktree");
    for directory in [&shared_target, &shared_target.join("runner")] {
        if let Err(error) = fs::create_dir_all(directory) {
            let _ = fs::remove_dir_all(&cache);
            let _ = fs::remove_dir(&session);
            return Err(format!(
                "无法创建 replay Cargo target {}：{error}",
                directory.display()
            ));
        }
    }
    Ok(ReplayWorkspace {
        session,
        cache,
        shared_target,
        shared_worktree,
    })
}
pub(super) fn cleanup_replay(
    context: &ReplayContext<'_>,
    allowed: &Path,
    sccache_started: bool,
) -> Vec<String> {
    let mut errors = Vec::new();
    if sccache_started
        && let Some(error) = stop_sccache(&context.tools.executables.sccache, context.environment)
    {
        errors.push(error);
    }
    errors.extend(cleanup_created_worktrees(
        &[
            (
                context.workspace.shared_worktree.join("f").exists(),
                context.frontend.root.clone(),
                context.workspace.shared_worktree.join("f"),
                "前端",
            ),
            (
                context.workspace.shared_worktree.join("b").exists(),
                context.backend.root.clone(),
                context.workspace.shared_worktree.join("b"),
                "后端",
            ),
        ],
        &context.workspace.shared_worktree,
    ));
    errors.extend(prune_worktrees(&[
        &context.backend.root,
        &context.frontend.root,
    ]));
    errors.extend(remove_isolated_tree(&context.workspace.cache, allowed));
    errors.extend(remove_isolated_tree(
        &context.workspace.shared_target,
        allowed,
    ));
    errors.extend(remove_isolated_tree(
        &context.workspace.shared_worktree,
        allowed,
    ));
    if context.workspace.session.is_dir()
        && let Err(error) = fs::remove_dir(&context.workspace.session)
    {
        errors.push(format!(
            "replay 临时目录未完全回收：{}：{error}",
            context.workspace.session.display()
        ));
    }
    errors
}
