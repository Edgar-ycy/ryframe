use std::{
    collections::BTreeSet,
    fs,
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
    let pipeline_build_dir = safe_target_directory(&plan.target_dir.join("gen"))?;
    println!("→ 编译候选模型并生成整批 CRUD 与迁移");
    let mut backend_allowed = match backend::generate(plan, &backend.path, tables, &catalog_target)
    {
        Ok(allowed) => allowed,
        Err(error) => return Err(backend.retain(error)),
    };
    let pipeline_target = match create_pipeline_target(&pipeline_build_dir) {
        Ok(target) => target,
        Err(error) => return Err(backend.retain(error)),
    };
    let (frontend, frontend_allowed) = match validate_candidate(
        &backend.path,
        frontend_root.as_deref(),
        pipeline_target.path(),
        &pipeline_build_dir,
        &catalog_target,
    ) {
        Ok(checked) => checked,
        Err(error) => return Err(backend.retain(error)),
    };
    if frontend.is_some() {
        backend_allowed.insert("openapi/openapi.json".into());
    }
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
    let target_path = pipeline_target.path().to_owned();
    if let Err(error) = pipeline_target.close() {
        eprintln!("候选产物目录 {} 清理失败：{error}", target_path.display());
    }
    backend_lock.release()
}

fn create_pipeline_target(build_dir: &Path) -> Result<tempfile::TempDir, String> {
    // 保留共享编译缓存，每次候选只运行自己目录里的最终产物。
    fs::create_dir_all(build_dir)
        .and_then(|()| {
            tempfile::Builder::new()
                .prefix("run-")
                .tempdir_in(build_dir)
        })
        .map_err(|error| {
            format!(
                "无法创建候选产物目录 {}：{error}；本批未安装",
                build_dir.display()
            )
        })
}

fn validate_candidate(
    backend: &Path,
    frontend_root: Option<&Path>,
    target: &Path,
    build_dir: &Path,
    catalog_target: &Path,
) -> Result<(Option<CandidateWorkspace>, BTreeSet<PathBuf>), String> {
    if let Some(root) = frontend_root {
        let frontend = CandidateWorkspace::create(root, false)?;
        match frontend::generate(backend, &frontend, root, target, build_dir, catalog_target) {
            Ok(allowed) => Ok((Some(frontend), allowed)),
            Err(error) => Err(frontend.retain(error)),
        }
    } else {
        preflight::validate(backend, target, build_dir)?;
        Ok((None, BTreeSet::new()))
    }
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
