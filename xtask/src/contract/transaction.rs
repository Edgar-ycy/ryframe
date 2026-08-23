use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use crate::Result;

use super::{
    candidate::read_optional,
    model::{ContractFileOperations, RealContractFileOperations, Snapshot, nonce},
};

struct ContractTransactionMarker {
    path: PathBuf,
}

impl ContractTransactionMarker {
    fn begin(paths: &[PathBuf], identity: &str) -> Result<Self> {
        let directory = contract_transaction_directory(paths)?;
        let path = directory.join(format!(".xtask-contract-transaction-{identity}"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(file, "pid={}", process::id())?;
        writeln!(file, "targets={}", paths.len())?;
        file.sync_all()?;
        Ok(Self { path })
    }

    fn finish(&mut self, operations: &impl ContractFileOperations) -> Result<()> {
        operations.remove_file(&self.path).map_err(|error| {
            format!(
                "契约事务已完成，但清理事务标记 {} 失败：{error}",
                self.path.display()
            )
            .into()
        })
    }
}

pub(super) fn install_snapshots(before: &[Snapshot], desired: &[Snapshot]) -> Result<()> {
    install_snapshots_with(before, desired, &RealContractFileOperations)
}

pub(crate) fn install_snapshots_with(
    before: &[Snapshot],
    desired: &[Snapshot],
    operations: &impl ContractFileOperations,
) -> Result<()> {
    if before.len() != desired.len()
        || before
            .iter()
            .zip(desired)
            .any(|(old, new)| old.path != new.path)
    {
        return Err("契约事务的新旧文件集合不一致".into());
    }
    let paths = before
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect::<Vec<_>>();
    reject_contract_recovery_artifacts(&paths)?;
    if before
        .iter()
        .zip(desired)
        .all(|(old, new)| old.content == new.content)
    {
        return Ok(());
    }
    let marker = format!("{}-{}", process::id(), nonce()?);
    let mut transaction = ContractTransactionMarker::begin(&paths, &marker)?;
    let mut staged = Vec::with_capacity(desired.len());
    for (index, (old, snapshot)) in before.iter().zip(desired).enumerate() {
        if old.content == snapshot.content {
            staged.push(None);
            continue;
        }
        let Some(content) = &snapshot.content else {
            staged.push(None);
            continue;
        };
        let stage_result: Result<PathBuf> = (|| {
            let path = contract_sibling_path(&snapshot.path, "new", &marker, index)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            if let Err(error) = output.write_all(content).and_then(|_| output.sync_all()) {
                drop(output);
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
            Ok(path)
        })();
        match stage_result {
            Ok(path) => staged.push(Some(path)),
            Err(error) => {
                let cleanup_errors =
                    cleanup_contract_files_with(staged.iter().flatten(), operations);
                if cleanup_errors.is_empty() {
                    transaction.finish(operations)?;
                    return Err(error);
                }
                return Err(format!(
                    "{error}；清理已暂存契约文件同时失败：{}；事务标记保留在 {}",
                    cleanup_errors.join("；"),
                    transaction.path.display()
                )
                .into());
            }
        }
    }

    let mut committed = Vec::new();
    for (index, (old, new)) in before.iter().zip(desired).enumerate() {
        let current = match fs::read(&old.path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
        };
        if current != old.content {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                format!("最终替换前文件发生变化，拒绝覆盖：{}", old.path.display()).into(),
                operations,
                &mut transaction,
            );
        }
        if old.content == new.content {
            continue;
        }

        let backup = if old.content.is_some() {
            let backup = match contract_sibling_path(&old.path, "backup", &marker, index) {
                Ok(path) => path,
                Err(error) => {
                    return rollback_snapshot_install(
                        before,
                        desired,
                        &staged,
                        &committed,
                        error,
                        operations,
                        &mut transaction,
                    );
                }
            };
            if let Err(error) = operations.rename(&old.path, &backup) {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
            committed.push((index, Some(backup.clone())));
            if fs::read(&backup).ok() != old.content {
                return rollback_snapshot_install(
                    before,
                    desired,
                    &staged,
                    &committed,
                    format!(
                        "原契约移入备份后内容发生变化，拒绝继续安装：{}",
                        backup.display()
                    )
                    .into(),
                    operations,
                    &mut transaction,
                );
            }
            Some(backup)
        } else {
            committed.push((index, None));
            None
        };
        let _ = backup;
        if new.content.is_some()
            && let Err(error) = operations.hard_link(
                staged[index]
                    .as_ref()
                    .expect("有目标内容的契约快照必须已经暂存"),
                &new.path,
            )
        {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
        if let Some(stage) = &staged[index]
            && let Err(error) = operations.remove_file(stage)
        {
            return rollback_snapshot_install(
                before,
                desired,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
    }

    for snapshot in desired {
        if read_optional(&snapshot.path)? != snapshot.content {
            return Err(format!(
                "契约安装后文件被并发修改，已保留当前内容、事务备份和标记 {}：{}",
                transaction.path.display(),
                snapshot.path.display()
            )
            .into());
        }
    }
    for (index, backup) in &committed {
        if let Some(backup) = backup
            && fs::read(backup).ok() != before[*index].content
        {
            return Err(format!(
                "契约已安装，但原文件备份 {} 被并发修改；已保留备份和事务标记 {}，拒绝清理",
                backup.display(),
                transaction.path.display()
            )
            .into());
        }
    }
    let backups = committed
        .iter()
        .filter_map(|(_, backup)| backup.as_ref())
        .collect::<Vec<_>>();
    let cleanup_errors = cleanup_contract_files_with(backups.into_iter(), operations);
    if cleanup_errors.is_empty() {
        transaction.finish(operations)
    } else {
        Err(format!(
            "契约文件已完整写入，但清理事务备份失败：{}；事务标记保留在 {}，下次同步会安全拒绝继续",
            cleanup_errors.join("；"),
            transaction.path.display()
        )
        .into())
    }
}

fn rollback_snapshot_install(
    before: &[Snapshot],
    desired: &[Snapshot],
    staged: &[Option<PathBuf>],
    committed: &[(usize, Option<PathBuf>)],
    cause: Box<dyn std::error::Error>,
    operations: &impl ContractFileOperations,
    transaction: &mut ContractTransactionMarker,
) -> Result<()> {
    let mut errors = Vec::new();
    for (index, backup) in committed.iter().rev() {
        let target = &before[*index].path;
        let current = match read_optional(target) {
            Ok(current) => current,
            Err(error) => {
                errors.push(format!("读取 {} 失败：{error}", target.display()));
                continue;
            }
        };
        if current == desired[*index].content {
            if current.is_some()
                && let Err(error) = operations.remove_file(target)
                && error.kind() != io::ErrorKind::NotFound
            {
                errors.push(format!("删除半写入文件 {} 失败：{error}", target.display()));
                continue;
            }
        } else if current.is_some() {
            errors.push(format!(
                "{} 在替换后被再次修改，已保留当前内容和事务备份",
                target.display()
            ));
            continue;
        }
        if let Some(backup) = backup
            && let Err(error) = operations.rename(backup, target)
        {
            errors.push(format!(
                "恢复 {} 失败，原文件备份保留在 {}：{error}",
                target.display(),
                backup.display()
            ));
        }
    }
    errors.extend(cleanup_contract_files_with(
        staged.iter().flatten(),
        operations,
    ));
    if errors.is_empty() {
        if let Err(error) = transaction.finish(operations) {
            return Err(format!("{cause}；{error}").into());
        }
        Err(cause)
    } else {
        Err(format!(
            "{cause}；契约事务回滚未能安全完成：{}；事务标记保留在 {}",
            errors.join("；"),
            transaction.path.display()
        )
        .into())
    }
}

fn contract_transaction_directory(paths: &[PathBuf]) -> Result<&Path> {
    paths
        .iter()
        .find(|path| {
            path.file_name().and_then(|value| value.to_str()) == Some("candidate.json")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|value| value.to_str())
                    == Some("openapi")
        })
        .or_else(|| paths.first())
        .and_then(|path| path.parent())
        .ok_or_else(|| "契约事务没有可用的标记目录".into())
}

fn contract_sibling_path(path: &Path, role: &str, marker: &str, index: usize) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("契约文件名不是 UTF-8：{}", path.display()))?;
    Ok(path.with_file_name(format!(".{name}.xtask-{role}-{marker}-{index}")))
}

pub(super) fn reject_contract_recovery_artifacts(paths: &[PathBuf]) -> Result<()> {
    let mut artifacts = Vec::new();
    let mut parents = Vec::new();
    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| format!("契约文件没有父目录：{}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("契约文件名不是 UTF-8：{}", path.display()))?;
        let prefix = format!(".{name}.xtask-");
        parents.push(parent.to_path_buf());
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    value
                        .strip_prefix(&prefix)
                        .is_some_and(is_contract_file_artifact)
                })
            {
                artifacts.push(candidate);
            }
        }
    }
    parents.sort();
    parents.dedup();
    for parent in parents {
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let candidate = entry?.path();
            if candidate
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix(".xtask-contract-transaction-"))
                .is_some_and(is_contract_identity)
            {
                artifacts.push(candidate);
            }
        }
    }
    artifacts.sort();
    artifacts.dedup();
    if artifacts.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "检测到上次契约事务未完整结束，已拒绝继续写入：{}；请根据 backup 恢复或确认目标已完整写入后再清理这些精确文件",
            artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("；")
        )
        .into())
    }
}

fn is_contract_file_artifact(value: &str) -> bool {
    let mut parts = value.split('-');
    matches!(parts.next(), Some("new" | "backup"))
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.all(is_contract_number)
}

fn is_contract_identity(value: &str) -> bool {
    let mut parts = value.split('-');
    parts.next().is_some_and(is_contract_number)
        && parts.next().is_some_and(is_contract_number)
        && parts.next().is_none()
}

fn is_contract_number(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn cleanup_contract_files_with<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    operations: &impl ContractFileOperations,
) -> Vec<String> {
    paths
        .filter_map(|path| match operations.remove_file(path) {
            Ok(()) => None,
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => Some(format!("{}：{error}", path.display())),
        })
        .collect()
}

fn ensure_atomic_write_has_no_recovery_artifacts(path: &Path) -> Result<()> {
    reject_contract_recovery_artifacts(&[path.to_path_buf()])
}

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
    ensure_atomic_write_has_no_recovery_artifacts(path)?;
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
