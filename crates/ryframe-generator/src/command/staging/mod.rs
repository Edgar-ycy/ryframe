use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::GenerationPlan;
use crate::import::TableInfo;
use lock::GenerationLock;
use snapshot::{FileChange, safe_path};
use workspace::CandidateWorkspace;

mod backend;
pub(super) mod bootstrap;
mod frontend;
mod install;
mod lock;
mod manifest;
mod preflight;
mod snapshot;
mod workspace;

pub(super) fn execute(plan: &GenerationPlan, tables: Option<&[TableInfo]>) -> Result<(), String> {
    let backend_lock = GenerationLock::acquire(safe_path(
        &plan.workspace_root,
        Path::new(".local-tests/business-generator.lock"),
    )?)?;
    let frontend_root = frontend_root(plan)?;
    let frontend_lock = frontend_root
        .as_ref()
        .map(|root| {
            GenerationLock::acquire(safe_path(root, Path::new("openapi/.xtask-contract.lock"))?)
        })
        .transpose()?;
    println!("→ 创建隔离候选工作区，正式源码保持前像");
    let backend = CandidateWorkspace::create(&plan.workspace_root, true)?;
    let cache = plan.target_dir.join("ryframe-generate");
    let catalog_target = cache.join("catalog-target");
    let pipeline_target = cache.join("pipeline");
    println!("→ 编译候选模型并生成整批 CRUD 与迁移");
    let mut backend_allowed = backend::generate(plan, &backend.path, tables, &catalog_target)?;
    let mut frontend_allowed = BTreeSet::new();
    let frontend = if let Some(root) = &frontend_root {
        let frontend = CandidateWorkspace::create(root, false)?;
        frontend_allowed = frontend::generate(
            &backend.path,
            &frontend,
            root,
            &pipeline_target,
            &catalog_target,
        )?;
        backend_allowed.insert("openapi/openapi.json".into());
        Some(frontend)
    } else {
        preflight::validate(&backend.path, &pipeline_target)?;
        None
    };
    let backend_changes = backend.changes(&backend_allowed)?;
    let frontend_changes = frontend
        .as_ref()
        .map(|candidate| candidate.changes(&frontend_allowed))
        .transpose()?;
    backend.verify_external()?;
    print_changes("后端", &backend_changes);
    if let Some(changes) = &frontend_changes {
        print_changes("前端", changes);
    }
    if plan.write {
        println!("→ 核对全部源码前像并统一安装生成结果");
        let mut stages = vec![install::prepare(&backend.source, backend_changes)?];
        if let (Some(frontend), Some(changes)) = (&frontend, frontend_changes) {
            stages.push(install::prepare(&frontend.source, changes)?);
        }
        install::commit(stages)?;
        println!("整批生成已安装；未执行建表、迁移或已有表接管。");
    } else {
        backend.source.verify()?;
        if let Some(frontend) = &frontend {
            frontend.source.verify()?;
        }
        println!("整批候选已校验，正式源码未写入；审阅上述差异后加 --write。");
    }
    backend.close()?;
    if let Some(frontend) = frontend {
        frontend.close()?;
    }
    if let Some(frontend_lock) = frontend_lock {
        frontend_lock.release()?;
    }
    backend_lock.release()
}

pub(super) fn safe_target_directory(target: &Path) -> Result<PathBuf, String> {
    let anchor = target
        .ancestors()
        .last()
        .ok_or("生成器 target_dir 缺少路径根")?;
    let relative = target
        .strip_prefix(anchor)
        .map_err(|_| "生成器 target_dir 路径无效")?;
    let target = safe_path(anchor, relative)?;
    if target
        .ancestors()
        .any(|path| path.exists() && !path.is_dir())
    {
        return Err("generator.target_dir 及已存在的父路径必须为普通目录".into());
    }
    Ok(target)
}

fn frontend_root(plan: &GenerationPlan) -> Result<Option<PathBuf>, String> {
    if !plan.sync_frontend {
        return Ok(None);
    }
    let root = plan.settings.frontend_dir(&plan.workspace_root);
    if !root.join("package.json").is_file() {
        return Err(format!(
            "前端目录无效：{}；修改 workspace.frontend_dir 或关闭 generator.sync_frontend；本批未安装",
            root.display()
        ));
    }
    for binary in [
        "node_modules/vue-tsc/bin/vue-tsc.js",
        "node_modules/eslint/bin/eslint.js",
    ] {
        frontend::tool(&root, Path::new(binary))?;
    }
    Ok(Some(root))
}

fn print_changes(label: &str, changes: &[FileChange]) {
    for change in changes {
        let action = match (&change.before, &change.after) {
            (None, _) => "create",
            (_, None) => "remove",
            _ => "update",
        };
        println!("{label} {action} {}", change.relative.display());
    }
    println!("{label}变更 {} 个文件。", changes.len());
}
