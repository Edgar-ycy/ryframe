use std::{
    fs,
    path::{Path, PathBuf},
};

use super::model::{
    CONTROL_GENERATED_PREFIX, CONTROL_PREFIX, Migration, TENANT_GENERATED_PREFIX, TENANT_PREFIX,
    migration_name, relative_posix,
};

pub(super) fn discover(root: &Path) -> Result<Vec<Migration>, String> {
    let control_dir = root.join(CONTROL_PREFIX);
    let tenant_dir = root.join(TENANT_PREFIX);
    let mut migrations = Vec::new();
    for entry in read_directory(&control_dir)? {
        let name = utf8_name(&entry)?;
        if entry.is_dir() && migration_name(&name) {
            migrations.push(Migration {
                name,
                source: entry.join("mod.rs"),
                module_registry: control_dir.join("mod.rs"),
                migrator_registry: control_dir.join("mod.rs"),
            });
        } else if entry.is_file()
            && entry.extension().and_then(|value| value.to_str()) == Some("rs")
        {
            let stem = entry
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if migration_name(stem) {
                migrations.push(Migration {
                    name: stem.to_owned(),
                    source: entry,
                    module_registry: control_dir.join("mod.rs"),
                    migrator_registry: control_dir.join("mod.rs"),
                });
            }
        }
    }
    for entry in read_directory(&tenant_dir)? {
        if !entry.is_file() || entry.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let stem = entry
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if migration_name(stem) {
            migrations.push(Migration {
                name: stem.to_owned(),
                source: entry,
                module_registry: tenant_dir.join("mod.rs"),
                migrator_registry: tenant_dir.join("runtime.rs"),
            });
        }
    }
    migrations.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(migrations)
}

pub(super) fn source_files(root: &Path, migration: &Migration) -> Result<Vec<PathBuf>, String> {
    let is_directory_entry = migration
        .source
        .file_name()
        .and_then(|value| value.to_str())
        == Some("mod.rs")
        && migration
            .source
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            == Some(migration.name.as_str());
    if !is_directory_entry {
        return Ok(vec![migration.source.clone()]);
    }
    let mut paths = Vec::new();
    collect_rust_files(
        migration
            .source
            .parent()
            .ok_or_else(|| format!("迁移入口缺少父目录：{}", migration.source.display()))?,
        &mut paths,
    )?;
    paths.sort_by_key(|path| relative_posix(root, path).unwrap_or_default());
    Ok(paths)
}

fn collect_rust_files(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in read_directory(directory)? {
        if entry.is_dir() {
            collect_rust_files(&entry, paths)?;
        } else if entry.is_file()
            && entry.extension().and_then(|value| value.to_str()) == Some("rs")
        {
            paths.push(entry);
        }
    }
    Ok(())
}

pub(super) fn discover_generated(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for prefix in [CONTROL_GENERATED_PREFIX, TENANT_GENERATED_PREFIX] {
        let directory = root.join(prefix);
        if !directory.is_dir() {
            continue;
        }
        for entry in read_directory(&directory)? {
            let migration = entry.join("migration.rs");
            if entry.is_dir() && migration.is_file() {
                paths.push(migration);
            }
        }
    }
    paths.sort_by_key(|path| relative_posix(root, path).unwrap_or_default());
    Ok(paths)
}

pub(crate) fn verify_generated_registry_wiring(root: &Path) -> Vec<String> {
    let required = [
        (
            "crates/ryframe-db/src/lib.rs",
            "pub mod generated;",
            "控制库 crate 必须一次性导出 generated 模块",
        ),
        (
            "crates/ryframe-db/src/generated/mod.rs",
            "pub const MIGRATION_NAMES: &[&str]",
            "控制库 generated 聚合必须提供只读迁移名称",
        ),
        (
            "crates/ryframe-db/src/generated/mod.rs",
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>>",
            "控制库 generated 聚合必须提供 migrations()",
        ),
        (
            "crates/ryframe-db/src/migration/mod.rs",
            "pub fn expected_migration_names()",
            "控制库只读校验必须消费手写及生成迁移名称",
        ),
        (
            "crates/ryframe-db/src/migration/mod.rs",
            "migrations.extend(crate::generated::migrations());",
            "控制库 Migrator 必须消费 generated::migrations()",
        ),
        (
            "crates/ryframe-tenant-db/src/lib.rs",
            "pub mod generated;",
            "租户库 crate 必须一次性导出 generated 模块",
        ),
        (
            "crates/ryframe-tenant-db/src/generated/mod.rs",
            "pub const MIGRATION_NAMES: &[&str]",
            "租户库 generated 聚合必须提供只读迁移名称",
        ),
        (
            "crates/ryframe-tenant-db/src/generated/mod.rs",
            "pub fn migrations() -> Vec<Box<dyn MigrationTrait>>",
            "租户库 generated 聚合必须提供 migrations()",
        ),
        (
            "crates/ryframe-tenant-db/src/migration/mod.rs",
            "pub fn expected_migration_names()",
            "租户库只读校验必须消费手写及生成迁移名称",
        ),
        (
            "crates/ryframe-tenant-db/src/migration/runtime.rs",
            "migrations.extend(crate::generated::migrations());",
            "租户库运行时 Migrator 必须消费 generated::migrations()",
        ),
    ];
    let mut errors = Vec::new();
    for (relative, marker, message) in required {
        match fs::read_to_string(root.join(relative)) {
            Ok(source) if source.contains(marker) => {}
            Ok(_) => errors.push(format!("{message}：{relative}")),
            Err(error) => errors.push(format!("{message}：无法读取 {relative}：{error}")),
        }
    }
    errors
}

fn read_directory(path: &Path) -> Result<Vec<PathBuf>, String> {
    fs::read_dir(path)
        .map_err(|error| format!("无法读取迁移目录 {}：{error}", path.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("无法读取迁移目录 {}：{error}", path.display()))
        })
        .collect()
}

fn utf8_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("迁移路径不是 UTF-8：{}", path.display()))
}
