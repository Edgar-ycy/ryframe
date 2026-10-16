use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::{
    super::GenerationPlan, install, lock::GenerationLock, manifest::normalize_external_paths,
    print_changes, snapshot::safe_path, workspace::CandidateWorkspace,
};
use crate::business::{BusinessBootstrapOptions, bootstrap_business_package};

mod lockfile;

pub(in crate::command) fn execute(plan: &GenerationPlan, module: &str) -> Result<(), String> {
    let lock = GenerationLock::acquire(safe_path(
        &plan.workspace_root,
        Path::new(".local-tests/business-generator.lock"),
    )?)?;
    println!("→ 在隔离候选工作区创建业务骨架并锁定依赖");
    let candidate = CandidateWorkspace::create(&plan.workspace_root, false)?;
    let allowed = generate(&candidate, module, &plan.package)?;
    let changes = candidate.changes(&allowed)?;
    print_changes("后端", &changes);
    if plan.write {
        install::commit(vec![install::prepare(&candidate.source, changes)?])?;
        println!("业务骨架、进程注册与 Cargo.lock 已统一安装；未修改数据库。");
    } else {
        candidate.source.verify()?;
        println!("业务骨架与锁文件预览通过，正式源码未写入；审阅后加 --write。");
    }
    candidate.close()?;
    lock.release()
}

fn generate(
    candidate: &CandidateWorkspace,
    module: &str,
    package: &str,
) -> Result<BTreeSet<PathBuf>, String> {
    let report = bootstrap_business_package(BusinessBootstrapOptions {
        workspace_root: &candidate.path,
        module,
        write: true,
    })
    .map_err(|error| format!("候选业务骨架生成失败：{error}；本批未安装"))?;
    let manifests = candidate
        .source
        .input_paths()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .map(|relative| {
            let path = safe_path(&candidate.path, relative)?;
            let bytes = fs::read(&path)
                .map_err(|error| format!("无法读取候选清单 {}：{error}", path.display()))?;
            Ok((path, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let external = normalize_external_paths(&candidate.source, &candidate.path)?;
    lockfile::resolve(&candidate.path, package)?;
    // 展开路径仅供候选 Cargo 使用，交付清单保留原路径与本次业务注册。
    for (path, bytes) in manifests {
        fs::write(&path, bytes)
            .map_err(|error| format!("无法恢复候选清单 {}：{error}；本批未安装", path.display()))?;
    }
    for source in external {
        source.verify()?;
    }
    let mut allowed = report
        .created
        .iter()
        .chain(&report.updated)
        .map(PathBuf::from)
        .collect::<BTreeSet<_>>();
    allowed.insert("Cargo.lock".into());
    Ok(allowed)
}
