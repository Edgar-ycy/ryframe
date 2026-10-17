use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::Result;
use ryframe_config::release_stage;

use super::{
    append::verify_append_only,
    discover::{discover, discover_generated, source_files, verify_generated_registry_wiring},
    git,
    lock::{load_lock, lock_path, render_lock, verify_lock, write_lock_atomically},
    model::{
        CONTROL_BASELINE, LockDocument, LockedFile, TENANT_BASELINE, lock_identity, relative_posix,
        sha256,
    },
    trusted::verify_trusted_head_lock,
};

pub(crate) fn check(root: &Path, require_frozen: bool, trusted_ref: &str) -> Vec<String> {
    let stage = match release_stage(root, trusted_ref) {
        Ok(stage) => stage,
        Err(error) => return vec![error],
    };
    let lock_path = lock_path(root);
    let (document, entries, load_errors) = load_lock(root, &lock_path);
    let mut errors = if load_errors.is_empty() {
        verify_lock(root, &lock_path)
    } else {
        load_errors.clone()
    };
    let head_paths = match git::head_paths(root, trusted_ref) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(error);
            return errors;
        }
    };
    if load_errors.is_empty()
        && stage.stable
        && let Some(document) = document.as_ref()
    {
        errors.extend(verify_trusted_head_lock(
            root,
            document,
            &entries,
            &head_paths,
            trusted_ref,
        ));
    }
    errors.extend(verify_generated_registry_wiring(root));
    let locked_paths = entries
        .iter()
        .map(|entry| entry.relative.clone())
        .collect::<BTreeSet<_>>();
    errors.extend(verify_append_only(
        root,
        Some(&locked_paths),
        require_frozen,
        Some(&head_paths),
    ));
    errors
}

pub(crate) fn refresh_baseline(root: &Path) -> Vec<String> {
    match release_stage(root, "HEAD") {
        Ok(stage) if stage.stable => {
            return vec!["稳定版阶段禁止重新锁定迁移基线".to_owned()];
        }
        Ok(_) => {}
        Err(error) => return vec![error],
    }
    let lock_path = lock_path(root);
    let (document, _entries, errors) = load_lock(root, &lock_path);
    if !errors.is_empty() || document.is_none() {
        return errors;
    }
    let mut errors = verify_generated_registry_wiring(root);
    errors.extend(verify_append_only(root, None, false, None));
    if !errors.is_empty() {
        return errors;
    }
    let mut paths = match discover(root) {
        Ok(migrations) => {
            let mut paths = Vec::new();
            for migration in migrations {
                match source_files(root, &migration) {
                    Ok(files) => paths.extend(files),
                    Err(error) => errors.push(error),
                }
            }
            paths
        }
        Err(error) => return vec![error],
    };
    match discover_generated(root) {
        Ok(generated) => paths.extend(generated),
        Err(error) => errors.push(error),
    }
    if !errors.is_empty() {
        return errors;
    }
    paths.sort_by_key(|path| relative_posix(root, path).unwrap_or_default());
    let mut entries = Vec::new();
    for path in paths {
        match locked_file(root, &path) {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return errors;
    }
    let before = match fs::read(&lock_path) {
        Ok(before) => before,
        Err(error) => return vec![format!("无法复核迁移冻结清单：{error}")],
    };
    let document = document.expect("已确认迁移冻结清单文档存在");
    let content = match render_lock(&document, &entries) {
        Ok(content) => content,
        Err(error) => return vec![error],
    };
    write_lock_atomically(&lock_path, &before, &content)
        .err()
        .into_iter()
        .collect()
}

pub(crate) fn freeze(root: &Path) -> Vec<String> {
    let state = match freeze_state(root) {
        Ok(state) => state,
        Err(errors) => return errors,
    };
    let mut errors = Vec::new();
    let mut additions = freeze_migration_additions(root, &state, &mut errors);
    additions.extend(freeze_generated_additions(root, &state, &mut errors));
    if !errors.is_empty() {
        return errors;
    }
    if additions.is_empty() {
        println!("没有待冻结的新迁移。");
        return Vec::new();
    }
    commit_freeze(&state, additions)
}

struct FreezeState {
    lock_path: PathBuf,
    document: LockDocument,
    entries: Vec<LockedFile>,
    head_paths: BTreeSet<String>,
    locked_paths: BTreeSet<String>,
}

fn freeze_state(root: &Path) -> std::result::Result<FreezeState, Vec<String>> {
    let lock_path = lock_path(root);
    let (document, entries, load_errors) = load_lock(root, &lock_path);
    let Some(document) = document else {
        return Err(if load_errors.is_empty() {
            vec!["迁移冻结清单缺少文档".to_owned()]
        } else {
            load_errors
        });
    };
    if !load_errors.is_empty() {
        return Err(load_errors);
    }
    let mut errors = verify_lock(root, &lock_path);
    errors.extend(verify_generated_registry_wiring(root));
    let head_paths = match git::head_paths(root, "HEAD") {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(error);
            return Err(errors);
        }
    };
    errors.extend(verify_trusted_head_lock(
        root,
        &document,
        &entries,
        &head_paths,
        "HEAD",
    ));
    let locked_paths = entries
        .iter()
        .map(|entry| entry.relative.clone())
        .collect::<BTreeSet<_>>();
    errors.extend(verify_append_only(
        root,
        Some(&locked_paths),
        false,
        Some(&head_paths),
    ));
    if errors.is_empty() {
        Ok(FreezeState {
            lock_path,
            document,
            entries,
            head_paths,
            locked_paths,
        })
    } else {
        Err(errors)
    }
}

fn freeze_migration_additions(
    root: &Path,
    state: &FreezeState,
    errors: &mut Vec<String>,
) -> Vec<LockedFile> {
    let migrations = match discover(root) {
        Ok(migrations) => migrations,
        Err(error) => {
            errors.push(error);
            return Vec::new();
        }
    };
    let mut additions = Vec::new();
    for migration in migrations {
        if migration.name == CONTROL_BASELINE || migration.name == TENANT_BASELINE {
            continue;
        }
        let files = match source_files(root, &migration) {
            Ok(files) => files,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let mut relatives = Vec::new();
        for path in &files {
            match relative_posix(root, path) {
                Ok(relative) => relatives.push(relative),
                Err(error) => errors.push(error),
            }
        }
        if relatives
            .iter()
            .all(|relative| state.locked_paths.contains(relative))
        {
            continue;
        }
        match fs::read_to_string(&migration.source) {
            Ok(source) if source.contains("尚未实现") => {
                errors.push(format!("迁移仍是未实现骨架，不能冻结：{}", migration.name));
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                errors.push(format!(
                    "无法读取迁移 {}：{error}",
                    migration.source.display()
                ));
                continue;
            }
        }
        collect_new_files(root, &files, &relatives, state, errors, &mut additions);
    }
    additions
}

fn freeze_generated_additions(
    root: &Path,
    state: &FreezeState,
    errors: &mut Vec<String>,
) -> Vec<LockedFile> {
    let generated = match discover_generated(root) {
        Ok(generated) => generated,
        Err(error) => {
            errors.push(error);
            return Vec::new();
        }
    };
    let mut additions = Vec::new();
    for path in generated {
        let relative = match relative_posix(root, &path) {
            Ok(relative) => relative,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if state.locked_paths.contains(&relative) {
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(source)
                if source.contains("INITIAL_RESOURCE_MIGRATION")
                    && !source.contains("尚未实现") => {}
            Ok(_) => {
                errors.push(format!("资源初始迁移不完整，不能冻结：{relative}"));
                continue;
            }
            Err(error) => {
                errors.push(format!("无法读取 generated migration {relative}：{error}"));
                continue;
            }
        }
        if state.head_paths.contains(&relative) {
            errors.push(format!("拒绝补录已存在于 HEAD 的历史迁移：{relative}"));
            continue;
        }
        match locked_file(root, &path) {
            Ok(entry) => additions.push(entry),
            Err(error) => errors.push(error),
        }
    }
    additions
}

fn collect_new_files(
    root: &Path,
    files: &[PathBuf],
    relatives: &[String],
    state: &FreezeState,
    errors: &mut Vec<String>,
    additions: &mut Vec<LockedFile>,
) {
    for (path, relative) in files.iter().zip(relatives) {
        if state.locked_paths.contains(relative) {
            continue;
        }
        if state.head_paths.contains(relative) {
            errors.push(format!("拒绝补录已存在于 HEAD 的历史迁移：{relative}"));
            continue;
        }
        match locked_file(root, path) {
            Ok(entry) => additions.push(entry),
            Err(error) => errors.push(error),
        }
    }
}

fn commit_freeze(state: &FreezeState, mut additions: Vec<LockedFile>) -> Vec<String> {
    additions.sort_by(|left, right| left.relative.cmp(&right.relative));
    let before = match fs::read(&state.lock_path) {
        Ok(before) => before,
        Err(error) => return vec![format!("无法复核迁移冻结清单：{error}")],
    };
    let mut updated_entries = state.entries.clone();
    updated_entries.extend(additions.iter().cloned());
    let content = match render_lock(&state.document, &updated_entries) {
        Ok(content) => content,
        Err(error) => return vec![error],
    };
    if let Err(error) = write_lock_atomically(&state.lock_path, &before, &content) {
        return vec![error];
    }
    println!("已冻结以下新迁移源码：");
    for entry in additions {
        println!("  - {}", entry.relative);
    }
    Vec::new()
}

pub(crate) fn run_check(root: &Path, require_frozen: bool, trusted_ref: &str) -> Result<()> {
    report(check(root, require_frozen, trusted_ref))?;
    if require_frozen {
        println!("迁移历史校验通过：源码、注册与迁移锁文件一致，版本阶段保护已校验。");
    } else {
        println!("迁移历史校验通过：冻结迁移未变化，工作树新迁移可继续编辑。");
    }
    Ok(())
}

pub(crate) fn run_refresh_baseline(root: &Path) -> Result<()> {
    report(refresh_baseline(root))?;
    println!("开发版迁移基线已刷新；现有数据库未改动，请使用新建隔离数据库验收。");
    Ok(())
}

pub(crate) fn run_freeze(root: &Path) -> Result<()> {
    report(freeze(root))?;
    println!("迁移冻结完成；从 v1.0.0 起只能通过追加迁移修复。");
    Ok(())
}

pub(crate) fn run_reconcile(root: &Path) -> Result<()> {
    match release_stage(root, "HEAD") {
        Ok(stage) if stage.stable => run_freeze(root),
        Ok(_) => run_refresh_baseline(root),
        Err(error) => Err(error.into()),
    }
}

fn locked_file(root: &Path, path: &Path) -> std::result::Result<LockedFile, String> {
    let relative = relative_posix(root, path)?;
    let (storage, target) = lock_identity(&relative)?;
    Ok(LockedFile {
        relative,
        path: path.to_path_buf(),
        expected_sha256: sha256(path)?,
        storage: storage.to_owned(),
        target,
    })
}

fn report(errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }
    let rendered = errors
        .into_iter()
        .map(|error| format!("  - {error}"))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("迁移历史校验失败：\n{rendered}").into())
}
