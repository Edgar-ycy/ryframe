use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use super::transaction::{
    ExpectedFile, InstalledFile, install_staged, move_to_backup, persist_recovery_directories,
    rollback, verify_expected_file, write_staged,
};
use super::{BusinessAsset, OWNERSHIP_PATH, ResourceError, error, hash};

#[derive(Default)]
struct Journal {
    backups: Vec<(PathBuf, PathBuf)>,
    installed: Vec<InstalledFile>,
}

pub(super) fn apply_changes(
    root: &Path,
    assets: &[BusinessAsset],
    removed: &[String],
    manifest: &str,
    expected: &BTreeMap<String, ExpectedFile>,
) -> Result<(), ResourceError> {
    let mut all_assets = assets.to_vec();
    all_assets.push(BusinessAsset {
        resource: "__catalog__".into(),
        path: OWNERSHIP_PATH.into(),
        content: manifest.into(),
    });
    apply_files(root, &all_assets, removed, expected)
}

/// 模型初稿与显式注册文件使用同一事务协议，但不登记为禁止人工编辑的生成产物。
pub(crate) fn apply_files(
    root: &Path,
    assets: &[BusinessAsset],
    removed: &[String],
    expected: &BTreeMap<String, ExpectedFile>,
) -> Result<(), ResourceError> {
    let paths = removed
        .iter()
        .map(String::as_str)
        .chain(assets.iter().map(|asset| asset.path.as_str()))
        .collect::<BTreeSet<_>>();
    for path in &paths {
        verify_expected_file(&safe_target(root, path)?, &expected[*path])?;
    }
    let stage = tempfile::Builder::new()
        .prefix(".ryframe-stage-")
        .tempdir_in(root)
        .map_err(|io| error(format!("无法创建同卷暂存目录：{io}"), root))?;
    for asset in assets {
        write_staged(
            &stage.path().join(&asset.path),
            &asset.content,
            &asset.resource,
        )?;
    }
    commit_files(root, stage, assets, &paths, expected)
}

pub(super) fn commit_files(
    root: &Path,
    stage: tempfile::TempDir,
    assets: &[BusinessAsset],
    paths: &BTreeSet<&str>,
    expected: &BTreeMap<String, ExpectedFile>,
) -> Result<(), ResourceError> {
    let mut journal = Journal::default();
    let result = install(root, stage.path(), assets, paths, expected, &mut journal);
    if let Err(failure) = result {
        if let Err(rollback_error) = rollback(&journal.installed, &journal.backups) {
            let saved = persist_recovery_directories(stage, None);
            return Err(error(
                format!("{failure}；回滚未完成：{rollback_error}；恢复目录：{saved:?}"),
                root,
            ));
        }
        return Err(failure);
    }
    Ok(())
}

fn install(
    root: &Path,
    stage: &Path,
    assets: &[BusinessAsset],
    paths: &BTreeSet<&str>,
    expected: &BTreeMap<String, ExpectedFile>,
    journal: &mut Journal,
) -> Result<(), ResourceError> {
    for path in paths {
        let target = safe_target(root, path)?;
        verify_expected_file(&target, &expected[*path])?;
        if expected[*path].exists() {
            let backup = stage.join(".backup").join(path);
            move_to_backup(&target, &backup)?;
            journal.backups.push((target, backup.clone()));
            verify_expected_file(&backup, &expected[*path])?;
        }
    }
    for asset in assets {
        let target = safe_target(root, &asset.path)?;
        install_staged(&stage.join(&asset.path), &target, &asset.resource)?;
        journal.installed.push(InstalledFile {
            path: target,
            content_hash: hash(asset.content.as_bytes()),
        });
    }
    Ok(())
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf, ResourceError> {
    let mut target = root.to_path_buf();
    for component in Path::new(relative).components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(error("生成路径不是受控相对路径", &target));
        }
        target.push(component);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(error("生成路径包含链接或重解析点", &target));
            }
            Ok(_) => {}
            Err(io) if io.kind() == std::io::ErrorKind::NotFound => {}
            Err(io) => return Err(error(format!("无法验证生成路径：{io}"), &target)),
        }
    }
    Ok(target)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
