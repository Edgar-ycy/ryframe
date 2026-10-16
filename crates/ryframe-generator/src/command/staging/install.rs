use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::snapshot::{FileChange, SourceSnapshot, safe_path};
use crate::resource::write_transaction::{
    ExpectedFile, InstalledFile, install_staged, move_to_backup, persist_recovery_directories,
    rollback, verify_expected_file, write_staged,
};

pub(super) struct PreparedInstallation<'a> {
    pub(super) source: &'a SourceSnapshot,
    pub(super) changes: Vec<FileChange>,
    pub(super) directory: tempfile::TempDir,
}

#[derive(Default)]
struct Journal {
    installed: Vec<InstalledFile>,
    backups: Vec<(PathBuf, PathBuf)>,
}

pub(super) fn prepare(
    source: &SourceSnapshot,
    changes: Vec<FileChange>,
) -> Result<PreparedInstallation<'_>, String> {
    let parent = safe_path(&source.root, Path::new(".local-tests"))?;
    fs::create_dir_all(&parent)
        .map_err(|error| format!("无法创建生成事务目录：{error}；本批未安装"))?;
    let directory = tempfile::Builder::new()
        .prefix("generation-install-")
        .tempdir_in(&parent)
        .map_err(|error| format!("无法创建同卷生成事务：{error}；本批未安装"))?;
    let mut paths = BTreeSet::new();
    for change in &changes {
        if change.relative.starts_with(".backup") {
            return Err("生成目标占用了事务备份目录；本批未安装".into());
        }
        safe_path(&source.root, &change.relative)?;
        if !paths.insert(change.relative.clone()) {
            return Err(format!(
                "生成事务重复路径：{}；本批未安装",
                change.relative.display()
            ));
        }
        if let Some(bytes) = &change.after {
            let content = std::str::from_utf8(bytes).map_err(|_| {
                format!(
                    "生成产物不是 UTF-8：{}；本批未安装",
                    change.relative.display()
                )
            })?;
            write_staged(
                &safe_path(directory.path(), &change.relative)?,
                content,
                "business",
            )
            .map_err(text_error)?;
        }
    }
    Ok(PreparedInstallation {
        source,
        changes,
        directory,
    })
}

/// 前后端共同使用一个回滚日志；写入前核对全部输入，再逐个保护目标前像。
pub(super) fn commit(stages: Vec<PreparedInstallation<'_>>) -> Result<(), String> {
    validate_roots(&stages)?;
    for stage in &stages {
        stage.source.verify()?;
    }
    let mut journal = Journal::default();
    let result =
        backup_all(&stages, &mut journal).and_then(|()| install_all(&stages, &mut journal));
    if let Err(error) = result {
        if let Err(recovery) = rollback(&journal.installed, &journal.backups) {
            let mut directories = stages.into_iter().map(|stage| stage.directory);
            let first = directories.next().expect("已核对一个或两个工作区");
            let saved = persist_recovery_directories(first, directories.next());
            return Err(format!(
                "{error}；回滚未完成：{recovery}；保留恢复目录：{saved:?}；核对正式文件与备份，不自动重放"
            ));
        }
        return Err(format!(
            "{error}；本批已回滚，保留并发写入，重新预览后再执行"
        ));
    }
    for stage in stages {
        let path = stage.directory.path().to_path_buf();
        stage.directory.close().map_err(|error| {
            format!(
                "生成文件已安装，但事务缓存 {} 清理失败：{error}；确认无占用后只清理该目录",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn validate_roots(stages: &[PreparedInstallation<'_>]) -> Result<(), String> {
    if stages.is_empty() || stages.len() > 2 {
        return Err("生成事务必须包含一个或两个独立工作区；未安装".into());
    }
    if stages.len() == 2
        && (stages[0].source.root.starts_with(&stages[1].source.root)
            || stages[1].source.root.starts_with(&stages[0].source.root))
    {
        return Err("前后端工作区不能重叠；本批未安装".into());
    }
    Ok(())
}

fn expected(change: &FileChange) -> ExpectedFile {
    change
        .before
        .as_ref()
        .map_or(ExpectedFile::Absent, |bytes| {
            ExpectedFile::ExactBytes(bytes.clone())
        })
}

fn backup_all(stages: &[PreparedInstallation<'_>], journal: &mut Journal) -> Result<(), String> {
    for stage in stages {
        for change in &stage.changes {
            let target = safe_path(&stage.source.root, &change.relative)?;
            let expected = expected(change);
            verify_expected_file(&target, &expected).map_err(text_error)?;
            if expected.exists() {
                let backup = stage
                    .directory
                    .path()
                    .join(".backup")
                    .join(&change.relative);
                move_to_backup(&target, &backup).map_err(text_error)?;
                journal.backups.push((target, backup.clone()));
                verify_expected_file(&backup, &expected).map_err(text_error)?;
            }
        }
    }
    Ok(())
}

fn install_all(stages: &[PreparedInstallation<'_>], journal: &mut Journal) -> Result<(), String> {
    for stage in stages {
        for change in &stage.changes {
            if let Some(bytes) = &change.after {
                let target = safe_path(&stage.source.root, &change.relative)?;
                let staged = stage.directory.path().join(&change.relative);
                let content_hash = hex::encode(Sha256::digest(bytes));
                let expected = ExpectedFile::ContentHash(content_hash.clone());
                verify_expected_file(&staged, &expected).map_err(text_error)?;
                install_staged(&staged, &target, "business").map_err(text_error)?;
                journal.installed.push(InstalledFile {
                    path: target.clone(),
                    content_hash,
                });
                verify_expected_file(&target, &expected).map_err(text_error)?;
            }
        }
    }
    Ok(())
}

fn text_error(error: crate::ResourceError) -> String {
    error.to_string()
}
