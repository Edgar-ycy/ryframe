use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Utc;

use crate::{
    Result,
    cli::{MigrationCommand, MigrationScope, MigrationTarget},
    process::run as run_process,
    workspace::root_dir,
};

const CONTROL_MIGRATION_DIR: &str = "crates/ryframe-db/src/migration";
const TENANT_MIGRATION_DIR: &str = "crates/ryframe-tenant-db/src/migration";

#[derive(Debug)]
pub(crate) struct PlannedWrite {
    pub(crate) path: PathBuf,
    pub(crate) expected: Option<Vec<u8>>,
    pub(crate) content: Vec<u8>,
}

pub(crate) trait FileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

struct RealFileOperations;

impl FileOperations for RealFileOperations {
    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::rename(source, target)
    }

    fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
        fs::hard_link(source, target)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

struct MigrationLock {
    path: PathBuf,
    identity: Vec<u8>,
}

impl MigrationLock {
    fn acquire(root: &Path, scope: MigrationScope) -> Result<Self> {
        let directory = migration_directory(root, scope);
        if !directory.is_dir() {
            return Err(format!("迁移目录不存在：{}", directory.display()).into());
        }
        let path = directory.join(".xtask-migration.lock");
        let identity =
            format!("pid={}\nnonce={}\n", process::id(), transaction_nonce()?).into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "无法获取迁移写入锁 {}：{error}。若确认没有其他 cargo migrate new 正在运行，请检查迁移事务残留后再删除该锁",
                    path.display()
                )
            })?;
        if let Err(error) = file.write_all(&identity).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, identity })
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        if fs::read(&self.path).is_ok_and(|content| content.as_slice() == self.identity.as_slice())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn migration_directory(root: &Path, scope: MigrationScope) -> PathBuf {
    match scope {
        MigrationScope::Control => root.join(CONTROL_MIGRATION_DIR),
        MigrationScope::TenantData => root.join(TENANT_MIGRATION_DIR),
    }
}

pub(crate) fn run(command: &MigrationCommand) -> Result<()> {
    match command {
        MigrationCommand::Freeze => run_process(
            &root_dir(),
            "python",
            &["scripts/check_migration_history.py", "--freeze"],
        ),
        MigrationCommand::Run { operation, target } => {
            let operation = operation.as_str();
            let mut migration_args = match target {
                MigrationTarget::Control => vec!["control", operation],
                MigrationTarget::TenantDataAll => vec!["tenant-data", operation, "--all"],
                MigrationTarget::TenantDataOne(target) => {
                    vec!["tenant-data", operation, "--target", target]
                }
            };
            let mut args = vec![
                "run",
                "--locked",
                "-p",
                "ryframe",
                "--bin",
                "ryframe-migrate",
                "--",
            ];
            args.append(&mut migration_args);
            run_process(&root_dir(), "cargo", &args)
        }
        MigrationCommand::New { scope, name } => {
            let _lock = MigrationLock::acquire(&root_dir(), *scope)?;
            let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
            create_migration_under_lock(&root_dir(), *scope, name, &timestamp)
        }
    }
}

#[allow(dead_code)]
pub(crate) fn create_migration(
    root: &Path,
    scope: MigrationScope,
    name: &str,
    timestamp: &str,
) -> Result<()> {
    let _lock = MigrationLock::acquire(root, scope)?;
    create_migration_under_lock(root, scope, name, timestamp)
}

fn create_migration_under_lock(
    root: &Path,
    scope: MigrationScope,
    name: &str,
    timestamp: &str,
) -> Result<()> {
    if !is_timestamp(timestamp) {
        return Err(format!("迁移时间戳无效：{timestamp}").into());
    }
    if !is_migration_name(name) {
        return Err("迁移名必须是 snake_case，且以小写英文字母开头".into());
    }
    let module = format!("m{timestamp}_{name}");
    let writes = plan_migration(root, scope, &module)?;
    commit_writes(&writes)?;
    println!("已创建追加迁移 {module}；执行 up 前必须补充迁移实现。");
    Ok(())
}

fn is_timestamp(value: &str) -> bool {
    value.len() == 15
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 8 && byte == b'_' || index != 8 && byte.is_ascii_digit())
}

fn is_migration_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn plan_migration(root: &Path, scope: MigrationScope, module: &str) -> Result<Vec<PlannedWrite>> {
    let (migration_dir, module_registry, migrator_registry, migrator_path) = match scope {
        MigrationScope::Control => (
            root.join(CONTROL_MIGRATION_DIR),
            root.join(CONTROL_MIGRATION_DIR).join("mod.rs"),
            root.join(CONTROL_MIGRATION_DIR).join("mod.rs"),
            format!("{module}::Migration"),
        ),
        MigrationScope::TenantData => (
            root.join(TENANT_MIGRATION_DIR),
            root.join(TENANT_MIGRATION_DIR).join("mod.rs"),
            root.join(TENANT_MIGRATION_DIR).join("runtime.rs"),
            format!("super::{module}::Migration"),
        ),
    };
    let migration_path = migration_dir.join(format!("{module}.rs"));
    ensure_direct_child(&migration_dir, &migration_path)?;
    ensure_newer_timestamp(&migration_dir, module)?;
    if migration_path.exists() {
        return Err(format!("迁移文件已存在，拒绝覆盖：{}", migration_path.display()).into());
    }

    let module_bytes = fs::read(&module_registry).map_err(|error| {
        format!(
            "无法读取迁移模块注册表 {}：{error}",
            module_registry.display()
        )
    })?;
    let module_source = String::from_utf8(module_bytes.clone())
        .map_err(|error| format!("迁移模块注册表不是 UTF-8：{error}"))?;
    let updated_module_source = insert_module_declaration(&module_source, module)?;

    let (migrator_bytes, migrator_source) = if migrator_registry == module_registry {
        (module_bytes.clone(), updated_module_source.clone())
    } else {
        let bytes = fs::read(&migrator_registry).map_err(|error| {
            format!(
                "无法读取 Migrator 注册表 {}：{error}",
                migrator_registry.display()
            )
        })?;
        let source = String::from_utf8(bytes.clone())
            .map_err(|error| format!("Migrator 注册表不是 UTF-8：{error}"))?;
        (bytes, source)
    };
    let updated_migrator_source = insert_migrator_entry(&migrator_source, &migrator_path)?;

    let mut writes = vec![PlannedWrite {
        path: migration_path,
        expected: None,
        content: render_migration(module).into_bytes(),
    }];
    if migrator_registry == module_registry {
        writes.push(PlannedWrite {
            path: module_registry,
            expected: Some(module_bytes),
            content: updated_migrator_source.into_bytes(),
        });
    } else {
        writes.push(PlannedWrite {
            path: module_registry,
            expected: Some(module_bytes),
            content: updated_module_source.into_bytes(),
        });
        writes.push(PlannedWrite {
            path: migrator_registry,
            expected: Some(migrator_bytes),
            content: updated_migrator_source.into_bytes(),
        });
    }
    Ok(writes)
}

fn ensure_newer_timestamp(migration_dir: &Path, module: &str) -> Result<()> {
    let timestamp = module
        .strip_prefix('m')
        .and_then(|value| value.get(..15))
        .ok_or_else(|| format!("迁移模块名无效：{module}"))?;
    let mut latest: Option<String> = None;
    for entry in fs::read_dir(migration_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = if path.is_dir() {
            entry.file_name().to_string_lossy().into_owned()
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            let Some(stem) = path.file_stem() else {
                continue;
            };
            stem.to_string_lossy().into_owned()
        } else {
            continue;
        };
        let Some(candidate) = name
            .strip_prefix('m')
            .and_then(|value| value.get(..15))
            .filter(|value| is_timestamp(value))
        else {
            continue;
        };
        if latest.as_deref().is_none_or(|value| candidate > value) {
            latest = Some(candidate.to_owned());
        }
    }
    if latest.as_deref().is_some_and(|value| timestamp <= value) {
        return Err(format!(
            "新迁移时间戳 {timestamp} 必须晚于现有最新迁移 {}；请等待下一秒后重试",
            latest.expect("已确认存在最新迁移")
        )
        .into());
    }
    Ok(())
}

fn ensure_direct_child(parent: &Path, child: &Path) -> Result<()> {
    if child.parent() != Some(parent) || child.file_name().is_none() {
        return Err(format!("迁移输出路径越过白名单目录：{}", child.display()).into());
    }
    if !parent.is_dir() {
        return Err(format!("迁移目录不存在：{}", parent.display()).into());
    }
    Ok(())
}

fn insert_module_declaration(source: &str, module: &str) -> Result<String> {
    let declaration = format!("mod {module};");
    if source.lines().any(|line| line.trim() == declaration) {
        return Err(format!("迁移模块已注册：{module}").into());
    }

    let mut insert_at = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("mod m") && trimmed.ends_with(';') {
            insert_at = Some(offset + line.len());
        }
        offset += line.len();
    }
    let insert_at = insert_at.ok_or("迁移模块注册表中未找到既有迁移声明")?;
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = source.to_owned();
    updated.insert_str(insert_at, &format!("{declaration}{newline}"));
    Ok(updated)
}

fn insert_migrator_entry(source: &str, migration_path: &str) -> Result<String> {
    if source.contains(migration_path) {
        return Err(format!("迁移已加入 Migrator：{migration_path}").into());
    }
    let function = source
        .find("fn migrations() -> Vec<Box<dyn MigrationTrait>>")
        .ok_or("未找到 MigratorTrait::migrations")?;
    let vector = source[function..]
        .find("vec![")
        .map(|offset| function + offset)
        .ok_or("MigratorTrait::migrations 未使用 vec! 注册迁移")?;
    let content_start = vector + "vec![".len();
    let content_end = source[content_start..]
        .find(']')
        .map(|offset| content_start + offset)
        .ok_or("Migrator 迁移向量缺少结束括号")?;
    let existing = source[content_start..content_end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(',').trim())
        .collect::<Vec<_>>();
    if existing.is_empty()
        || existing
            .iter()
            .any(|entry| !entry.starts_with("Box::new(") || !entry.ends_with(')'))
    {
        return Err("Migrator 迁移向量格式不受支持，拒绝自动改写".into());
    }

    let mut entries = existing.into_iter().map(str::to_owned).collect::<Vec<_>>();
    entries.push(format!("Box::new({migration_path})"));
    let rendered = format!(
        "\n            {},\n        ",
        entries.join(",\n            ")
    );
    let mut updated = source.to_owned();
    updated.replace_range(content_start..content_end, &rendered);
    Ok(updated)
}

fn render_migration(module: &str) -> String {
    format!(
        "use sea_orm_migration::prelude::*;\n\n\
         #[derive(DeriveMigrationName)]\n\
         pub struct Migration;\n\n\
         #[async_trait::async_trait]\n\
         impl MigrationTrait for Migration {{\n\
             async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {{\n\
                 Err(DbErr::Custom(\n\
                     \"迁移 {module} 尚未实现；请补充只向前执行的 up 逻辑\".into(),\n\
                 ))\n\
             }}\n\n\
             async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {{\n\
                 Err(DbErr::Custom(\n\
                     \"追加迁移 {module} 不支持 down；请新建追加迁移执行 roll-forward 修复\".into(),\n\
                 ))\n\
             }}\n\
         }}\n"
    )
}

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

fn commit_writes(writes: &[PlannedWrite]) -> Result<()> {
    commit_writes_with(writes, &RealFileOperations)
}

pub(crate) fn commit_writes_with(
    writes: &[PlannedWrite],
    operations: &impl FileOperations,
) -> Result<()> {
    let recovery_artifacts = find_recovery_artifacts(writes)?;
    if !recovery_artifacts.is_empty() {
        return Err(format!(
            "检测到上次迁移文件事务未完整结束，已拒绝继续写入：{}；请根据 backup 文件恢复或确认目标已完整写入后再清理这些精确文件",
            recovery_artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("；")
        )
        .into());
    }
    let nonce = transaction_nonce()?;
    let mut transaction = MigrationTransactionMarker::begin(writes, nonce)?;
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

    let mut committed: Vec<(usize, Option<PathBuf>)> = Vec::new();
    for (index, write) in writes.iter().enumerate() {
        let actual = match fs::read(&write.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return rollback(
                    writes,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
        };
        if actual != write.expected {
            return rollback(
                writes,
                &staged,
                &committed,
                format!("写入前文件发生变化，拒绝覆盖：{}", write.path.display()).into(),
                operations,
                &mut transaction,
            );
        }

        let backup = if write.expected.is_some() {
            let backup = match sibling_path(&write.path, "backup", nonce, index) {
                Ok(path) => path,
                Err(error) => {
                    return rollback(
                        writes,
                        &staged,
                        &committed,
                        error,
                        operations,
                        &mut transaction,
                    );
                }
            };
            if let Err(error) = operations.rename(&write.path, &backup) {
                return rollback(
                    writes,
                    &staged,
                    &committed,
                    error.into(),
                    operations,
                    &mut transaction,
                );
            }
            committed.push((index, Some(backup.clone())));
            if fs::read(&backup).ok() != write.expected {
                return rollback(
                    writes,
                    &staged,
                    &committed,
                    format!(
                        "原文件移入备份后内容发生变化，拒绝继续安装：{}",
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
        if let Err(error) = operations.hard_link(&staged[index], &write.path) {
            return rollback(
                writes,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
        if let Err(error) = operations.remove_file(&staged[index]) {
            return rollback(
                writes,
                &staged,
                &committed,
                error.into(),
                operations,
                &mut transaction,
            );
        }
    }

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
    for (index, backup) in &committed {
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

fn transaction_nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
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
