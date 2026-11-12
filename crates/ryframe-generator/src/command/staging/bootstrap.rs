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
    let allowed = match generate(&candidate, module, &plan.package, &plan.target_dir) {
        Ok(allowed) => allowed,
        Err(error) => return Err(candidate.retain(error)),
    };
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
    target: &Path,
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
    lockfile::resolve(&candidate.path, package, target)?;
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

#[cfg(test)]
mod tests {
    use super::super::snapshot::SourceSnapshot;
    use super::*;
    use crate::command::{GenerationBuilder, GenerationOperation};

    #[test]
    fn rejected_business_bootstrap_retains_candidate_without_installing() {
        let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&cache).unwrap();
        let root = tempfile::Builder::new()
            .prefix("bootstrap-retention-")
            .tempdir_in(&cache)
            .unwrap();
        let package = Path::new("crates/order-business/Cargo.toml");
        let manifest = b"[package]\nname = \"order-business\"\n";
        fs::create_dir_all(root.path().join(package.parent().unwrap())).unwrap();
        fs::write(root.path().join(package), manifest).unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            b"[workspace]\nmembers = [\"crates/order-business\"]\n",
        )
        .unwrap();
        let before = SourceSnapshot::capture(root.path()).unwrap();

        let error = GenerationBuilder::new(root.path())
            .operation(GenerationOperation::NewBusiness("order".into()))
            .write(true)
            .plan()
            .unwrap()
            .execute()
            .unwrap_err();

        assert!(
            error.contains("候选业务骨架生成失败") && error.contains("业务 crate 已存在"),
            "{error}"
        );
        assert!(error.contains("候选现场保留于"), "{error}");
        let mut entries = fs::read_dir(root.path().join(".local-tests")).unwrap();
        let retained = entries.next().unwrap().unwrap();
        assert!(entries.next().is_none());
        assert!(
            error.contains(retained.file_name().to_str().unwrap()),
            "{error}"
        );
        assert_eq!(
            fs::read(retained.path().join("workspace").join(package)).unwrap(),
            manifest
        );
        before.verify().unwrap();
    }
}
