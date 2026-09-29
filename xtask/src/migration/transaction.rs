use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use crate::Result;

use super::model::{FileOperations, PlannedWrite, RealFileOperations, transaction_nonce};

struct MigrationTransactionMarker {
    path: PathBuf,
}

impl MigrationTransactionMarker {
    fn begin(writes: &[PlannedWrite], nonce: u128) -> Result<Self> {
        let directory = transaction_directory(writes)?;
        let path = directory.join(format!(
            ".xtask-migration-transaction-{}-{nonce}",
            process::id()
        ));
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(marker, "pid={}", process::id())?;
        writeln!(marker, "targets={}", writes.len())?;
        marker.sync_all()?;
        Ok(Self { path })
    }

    fn finish(&mut self, operations: &impl FileOperations) -> Result<()> {
        operations.remove_file(&self.path).map_err(|error| {
            format!(
                "迁移事务已完成，但清理事务标记 {} 失败：{error}",
                self.path.display()
            )
            .into()
        })
    }
}

pub(super) fn commit_writes(writes: &[PlannedWrite]) -> Result<()> {
    commit_writes_with(writes, &RealFileOperations)
}

pub(crate) fn commit_writes_with(
    writes: &[PlannedWrite],
    operations: &impl FileOperations,
) -> Result<()> {
    reject_recovery_artifacts(writes)?;
    let nonce = transaction_nonce()?;
    let mut transaction = MigrationTransactionMarker::begin(writes, nonce)?;
    let staged = stage_migration_writes(writes, nonce, operations, &mut transaction)?;
    let committed = commit_migration_writes(writes, &staged, nonce, operations, &mut transaction)?;
    verify_migration_install(writes, &committed, &transaction)?;
    finish_migration_install(&committed, operations, &mut transaction)
}

fn reject_recovery_artifacts(writes: &[PlannedWrite]) -> Result<()> {
    let recovery_artifacts = find_recovery_artifacts(writes)?;
    if recovery_artifacts.is_empty() {
        return Ok(());
    }
    Err(format!(
        "检测到上次迁移文件事务未完整结束，已拒绝继续写入：{}；请根据 backup 文件恢复或确认目标已完整写入后再清理这些精确文件",
        recovery_artifacts
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("；")
    )
    .into())
}

fn stage_migration_writes(
    writes: &[PlannedWrite],
    nonce: u128,
    operations: &impl FileOperations,
    transaction: &mut MigrationTransactionMarker,
) -> Result<Vec<PathBuf>> {
    let mut staged = Vec::with_capacity(writes.len());
    for (index, write) in writes.iter().enumerate() {
        let stage_result: Result<PathBuf> = (|| {
            let stage = sibling_path(&write.path, "new", nonce, index)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)?;
            if let Err(error) = file.write_all(&write.content).and_then(|_| file.sync_all()) {
                let _ = fs::remove_file(&stage);
                return Err(error.into());
            }
            Ok(stage)
        })();
        match stage_result {
            Ok(stage) => staged.push(stage),
            Err(error) => {
                let cleanup_errors = cleanup_files_with(&staged, operations);
                if cleanup_errors.is_empty() {
                    transaction.finish(operations)?;
                    return Err(error);
                }
                return Err(format!(
                    "{error}；清理已暂存文件同时失败：{}；事务标记保留在 {}",
                    cleanup_errors.join("；"),
                    transaction.path.display()
                )
                .into());
            }
        }
    }
    Ok(staged)
}

fn commit_migration_writes(
    writes: &[PlannedWrite],
    staged: &[PathBuf],
    nonce: u128,
    operations: &impl FileOperations,
    transaction: &mut MigrationTransactionMarker,
) -> Result<Vec<(usize, Option<PathBuf>)>> {
    let mut committed: Vec<(usize, Option<PathBuf>)> = Vec::new();
    for (index, write) in writes.iter().enumerate() {
        commit_migration_write(
            writes,
            staged,
            nonce,
            index,
            write,
            &mut committed,
            operations,
            transaction,
        )?;
    }
    Ok(committed)
}

#[allow(clippy::too_many_arguments)]
fn commit_migration_write(
    writes: &[PlannedWrite],
    staged: &[PathBuf],
    nonce: u128,
    index: usize,
    write: &PlannedWrite,
    committed: &mut Vec<(usize, Option<PathBuf>)>,
    operations: &impl FileOperations,
    transaction: &mut MigrationTransactionMarker,
) -> Result<()> {
    let actual = match fs::read(&write.path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return rollback(
                writes,
                staged,
                committed,
                error.into(),
                operations,
                transaction,
            );
        }
    };
    if actual != write.expected {
        return rollback(
            writes,
            staged,
            committed,
            format!("写入前文件发生变化，拒绝覆盖：{}", write.path.display()).into(),
            operations,
            transaction,
        );
    }
    if write.expected.is_some() {
        let backup = match sibling_path(&write.path, "backup", nonce, index) {
            Ok(path) => path,
            Err(error) => {
                return rollback(writes, staged, committed, error, operations, transaction);
            }
        };
        if let Err(error) = operations.rename(&write.path, &backup) {
            return rollback(
                writes,
                staged,
                committed,
                error.into(),
                operations,
                transaction,
            );
        }
        committed.push((index, Some(backup.clone())));
        if fs::read(&backup).ok() != write.expected {
            let error = format!(
                "原文件移入备份后内容发生变化，拒绝继续安装：{}",
                backup.display()
            );
            return rollback(
                writes,
                staged,
                committed,
                error.into(),
                operations,
                transaction,
            );
        }
    } else {
        committed.push((index, None));
    }
    if let Err(error) = operations.hard_link(&staged[index], &write.path) {
        return rollback(
            writes,
            staged,
            committed,
            error.into(),
            operations,
            transaction,
        );
    }
    if let Err(error) = operations.remove_file(&staged[index]) {
        return rollback(
            writes,
            staged,
            committed,
            error.into(),
            operations,
            transaction,
        );
    }
    Ok(())
}

fn verify_migration_install(
    writes: &[PlannedWrite],
    committed: &[(usize, Option<PathBuf>)],
    transaction: &MigrationTransactionMarker,
) -> Result<()> {
    for write in writes {
        let installed = match fs::read(&write.path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if installed.as_deref() != Some(write.content.as_slice()) {
            return Err(format!(
                "迁移安装后文件被并发修改，已保留当前内容、事务备份和标记 {}：{}",
                transaction.path.display(),
                write.path.display()
            )
            .into());
        }
    }
    for (index, backup) in committed {
        if let Some(backup) = backup
            && fs::read(backup).ok() != writes[*index].expected
        {
            return Err(format!(
                "迁移已安装，但原文件备份 {} 被并发修改；已保留备份和事务标记 {}，拒绝清理",
                backup.display(),
                transaction.path.display()
            )
            .into());
        }
    }
    Ok(())
}

fn finish_migration_install(
    committed: &[(usize, Option<PathBuf>)],
    operations: &impl FileOperations,
    transaction: &mut MigrationTransactionMarker,
) -> Result<()> {
    let backup_paths = committed
        .iter()
        .filter_map(|(_, backup)| backup.clone())
        .collect::<Vec<_>>();
    let cleanup_errors = cleanup_files_with(&backup_paths, operations);
    if !cleanup_errors.is_empty() {
        return Err(format!(
            "迁移文件已写入，但清理备份失败：{}；目标文件保持新内容，事务标记保留在 {}",
            cleanup_errors.join("；"),
            transaction.path.display()
        )
        .into());
    }
    transaction.finish(operations)
}

fn transaction_directory(writes: &[PlannedWrite]) -> Result<&Path> {
    let directory = writes
        .first()
        .and_then(|write| write.path.parent())
        .ok_or("迁移写入集合为空或目标没有父目录")?;
    if writes
        .iter()
        .any(|write| write.path.parent() != Some(directory))
    {
        return Err("一次迁移事务的文件必须位于同一目录".into());
    }
    Ok(directory)
}

fn find_recovery_artifacts(writes: &[PlannedWrite]) -> Result<Vec<PathBuf>> {
    let directory = transaction_directory(writes)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut artifacts = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(is_migration_transaction_artifact)
        {
            artifacts.push(path);
        }
    }
    artifacts.sort();
    Ok(artifacts)
}

fn is_migration_transaction_artifact(name: &str) -> bool {
    if let Some(identity) = name.strip_prefix(".xtask-migration-transaction-") {
        let mut parts = identity.split('-');
        return parts.next().is_some_and(is_digits)
            && parts.next().is_some_and(is_digits)
            && parts.next().is_none();
    }
    let Some((target, operation)) = name.rsplit_once(".xtask-") else {
        return false;
    };
    if !target.starts_with('.') || target.len() == 1 {
        return false;
    }
    let mut parts = operation.split('-');
    matches!(parts.next(), Some("new" | "backup"))
        && parts.next().is_some_and(is_digits)
        && parts.next().is_some_and(is_digits)
        && parts.next().is_some_and(is_digits)
        && parts.next().is_none()
}

fn is_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn cleanup_files_with(paths: &[PathBuf], operations: &impl FileOperations) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| match operations.remove_file(path) {
            Ok(()) => None,
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => Some(format!("{}：{error}", path.display())),
        })
        .collect()
}

fn sibling_path(path: &Path, role: &str, nonce: u128, index: usize) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("输出文件名不是 UTF-8：{}", path.display()))?;
    Ok(path.with_file_name(format!(
        ".{name}.xtask-{role}-{}-{nonce}-{index}",
        process::id()
    )))
}

fn rollback(
    writes: &[PlannedWrite],
    staged: &[PathBuf],
    committed: &[(usize, Option<PathBuf>)],
    cause: Box<dyn std::error::Error>,
    operations: &impl FileOperations,
    transaction: &mut MigrationTransactionMarker,
) -> Result<()> {
    let mut rollback_errors = Vec::new();
    for (index, backup) in committed.iter().rev() {
        let target = &writes[*index].path;
        let current = match fs::read(target) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                rollback_errors.push(format!("读取 {} 失败：{error}", target.display()));
                continue;
            }
        };
        if current.as_deref() == Some(writes[*index].content.as_slice()) {
            if let Err(error) = operations.remove_file(target)
                && error.kind() != io::ErrorKind::NotFound
            {
                rollback_errors.push(format!("删除半写入文件 {} 失败：{error}", target.display()));
                continue;
            }
        } else if current.is_some() {
            rollback_errors.push(format!(
                "{} 在安装后被再次修改，已保留当前内容和事务备份",
                target.display()
            ));
            continue;
        }
        if let Some(backup) = backup
            && let Err(error) = operations.rename(backup, target)
        {
            rollback_errors.push(format!(
                "恢复文件 {} 失败（备份保留在 {}）：{error}",
                target.display(),
                backup.display()
            ));
        }
    }
    rollback_errors.extend(cleanup_files_with(staged, operations));
    if rollback_errors.is_empty() {
        if let Err(error) = transaction.finish(operations) {
            return Err(format!("{cause}；{error}").into());
        }
        Err(cause)
    } else {
        Err(format!(
            "{cause}；回滚未能安全完成：{}；事务标记保留在 {}",
            rollback_errors.join("；"),
            transaction.path.display()
        )
        .into())
    }
}
