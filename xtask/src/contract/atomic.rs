use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    process,
};

use crate::Result;

use super::{
    model::{ContractFileOperations, RealContractFileOperations, nonce},
    recovery::reject_contract_recovery_artifacts,
};

pub(super) fn write_atomically(path: &Path, content: &[u8]) -> Result<()> {
    write_atomically_with(path, content, &RealContractFileOperations)
}

pub(crate) fn write_atomically_with(
    path: &Path,
    content: &[u8],
    operations: &impl ContractFileOperations,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("文件没有父目录：{}", path.display()))?;
    fs::create_dir_all(parent)?;
    reject_contract_recovery_artifacts(&[path.to_path_buf()])?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("文件名不是 UTF-8：{}", path.display()))?;
    let marker = format!("{}-{}", process::id(), nonce()?);
    let staged = path.with_file_name(format!(".{name}.xtask-new-{marker}"));
    let backup = path.with_file_name(format!(".{name}.xtask-backup-{marker}"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    if let Err(error) = output.write_all(content).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = operations.remove_file(&staged);
        return Err(error.into());
    }
    drop(output);

    let had_original = path.exists();
    if had_original && let Err(error) = operations.rename(path, &backup) {
        let cleanup = operations.remove_file(&staged);
        return match cleanup {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(format!(
                "{error}；清理暂存文件 {} 同时失败：{cleanup_error}",
                staged.display()
            )
            .into()),
        };
    }
    if let Err(error) = operations.rename(&staged, path) {
        let mut recovery_errors = Vec::new();
        if had_original && let Err(restore_error) = operations.rename(&backup, path) {
            recovery_errors.push(format!(
                "恢复目标失败，原文件备份保留在 {}：{restore_error}",
                backup.display()
            ));
        }
        if let Err(cleanup_error) = operations.remove_file(&staged)
            && cleanup_error.kind() != io::ErrorKind::NotFound
        {
            recovery_errors.push(format!(
                "清理暂存文件 {} 失败：{cleanup_error}",
                staged.display()
            ));
        }
        return if recovery_errors.is_empty() {
            Err(error.into())
        } else {
            Err(format!("{error}；{}", recovery_errors.join("；")).into())
        };
    }
    if had_original && let Err(error) = operations.remove_file(&backup) {
        eprintln!(
            "警告：{} 已写入新内容，但清理备份 {} 失败：{error}；可确认后人工删除备份",
            path.display(),
            backup.display()
        );
    }
    Ok(())
}
