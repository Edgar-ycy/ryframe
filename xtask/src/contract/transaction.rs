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
    recovery::reject_contract_recovery_artifacts,
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

struct SnapshotInstallContext<'a, Operations> {
    before: &'a [Snapshot],
    desired: &'a [Snapshot],
    staged: &'a [Option<PathBuf>],
    operations: &'a Operations,
    transaction: &'a mut ContractTransactionMarker,
}

impl<Operations: ContractFileOperations> SnapshotInstallContext<'_, Operations> {
    fn rollback(
        &mut self,
        committed: &[(usize, Option<PathBuf>)],
        cause: Box<dyn std::error::Error>,
    ) -> Result<()> {
        rollback_snapshot_install(
            self.before,
            self.desired,
            self.staged,
            committed,
            cause,
            self.operations,
            self.transaction,
        )
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
    let paths = validate_snapshot_sets(before, desired)?;
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
    let staged = stage_contract_snapshots(before, desired, &marker, operations, &mut transaction)?;
    let committed = commit_contract_snapshots(
        before,
        desired,
        &staged,
        &marker,
        operations,
        &mut transaction,
    )?;
    verify_contract_install(before, desired, &committed, &transaction)?;
    finish_contract_install(&committed, operations, &mut transaction)
}

fn validate_snapshot_sets(before: &[Snapshot], desired: &[Snapshot]) -> Result<Vec<PathBuf>> {
    if before.len() != desired.len()
        || before
            .iter()
            .zip(desired)
            .any(|(old, new)| old.path != new.path)
    {
        return Err("契约事务的新旧文件集合不一致".into());
    }
    Ok(before
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect())
}

fn stage_contract_snapshots(
    before: &[Snapshot],
    desired: &[Snapshot],
    marker: &str,
    operations: &impl ContractFileOperations,
    transaction: &mut ContractTransactionMarker,
) -> Result<Vec<Option<PathBuf>>> {
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
            let path = contract_sibling_path(&snapshot.path, "new", marker, index)?;
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
    Ok(staged)
}

fn commit_contract_snapshots(
    before: &[Snapshot],
    desired: &[Snapshot],
    staged: &[Option<PathBuf>],
    marker: &str,
    operations: &impl ContractFileOperations,
    transaction: &mut ContractTransactionMarker,
) -> Result<Vec<(usize, Option<PathBuf>)>> {
    let mut committed = Vec::new();
    let mut context = SnapshotInstallContext {
        before,
        desired,
        staged,
        operations,
        transaction,
    };
    for (index, (old, new)) in before.iter().zip(desired).enumerate() {
        commit_contract_snapshot(&mut context, marker, index, old, new, &mut committed)?;
    }
    Ok(committed)
}

fn commit_contract_snapshot<Operations: ContractFileOperations>(
    context: &mut SnapshotInstallContext<'_, Operations>,
    marker: &str,
    index: usize,
    old: &Snapshot,
    new: &Snapshot,
    committed: &mut Vec<(usize, Option<PathBuf>)>,
) -> Result<()> {
    let current = match fs::read(&old.path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return context.rollback(committed, error.into());
        }
    };
    if current != old.content {
        return context.rollback(
            committed,
            format!("最终替换前文件发生变化，拒绝覆盖：{}", old.path.display()).into(),
        );
    }
    if old.content == new.content {
        return Ok(());
    }
    if old.content.is_some() {
        backup_contract_snapshot(context, marker, index, old, committed)?;
    } else {
        committed.push((index, None));
    }
    if new.content.is_some()
        && let Err(error) = context.operations.hard_link(
            context.staged[index]
                .as_ref()
                .expect("有目标内容的契约快照必须已经暂存"),
            &new.path,
        )
    {
        return context.rollback(committed, error.into());
    }
    if let Some(stage) = &context.staged[index]
        && let Err(error) = context.operations.remove_file(stage)
    {
        return context.rollback(committed, error.into());
    }
    Ok(())
}

fn backup_contract_snapshot<Operations: ContractFileOperations>(
    context: &mut SnapshotInstallContext<'_, Operations>,
    marker: &str,
    index: usize,
    old: &Snapshot,
    committed: &mut Vec<(usize, Option<PathBuf>)>,
) -> Result<()> {
    let backup = match contract_sibling_path(&old.path, "backup", marker, index) {
        Ok(path) => path,
        Err(error) => {
            return context.rollback(committed, error);
        }
    };
    if let Err(error) = context.operations.rename(&old.path, &backup) {
        return context.rollback(committed, error.into());
    }
    committed.push((index, Some(backup.clone())));
    if fs::read(&backup).ok() != old.content {
        let error = format!(
            "原契约移入备份后内容发生变化，拒绝继续安装：{}",
            backup.display()
        );
        return context.rollback(committed, error.into());
    }
    Ok(())
}

fn verify_contract_install(
    before: &[Snapshot],
    desired: &[Snapshot],
    committed: &[(usize, Option<PathBuf>)],
    transaction: &ContractTransactionMarker,
) -> Result<()> {
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
    for (index, backup) in committed {
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
    Ok(())
}

fn finish_contract_install(
    committed: &[(usize, Option<PathBuf>)],
    operations: &impl ContractFileOperations,
    transaction: &mut ContractTransactionMarker,
) -> Result<()> {
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
