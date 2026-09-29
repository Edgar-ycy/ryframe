use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;

use crate::{
    Result,
    cli::{MigrationCommand, MigrationOperation, MigrationScope, MigrationTarget},
    process::{run as run_process, run_owned},
    workspace::root_dir,
};

use super::{
    model::{MigrationLock, PlannedWrite},
    transaction::commit_writes,
};

const CONTROL_MIGRATION_DIR: &str = "crates/ryframe-db/src/migration";
const TENANT_MIGRATION_DIR: &str = "crates/ryframe-tenant-db/src/migration";

pub(super) fn migration_directory(root: &Path, scope: MigrationScope) -> PathBuf {
    match scope {
        MigrationScope::Control => root.join(CONTROL_MIGRATION_DIR),
        MigrationScope::TenantData => root.join(TENANT_MIGRATION_DIR),
    }
}

pub(crate) fn run(command: &MigrationCommand) -> Result<()> {
    match command {
        MigrationCommand::Baseline => run_process(
            &root_dir(),
            "python",
            &[
                "scripts/check_migration_history.py",
                "--refresh-baseline",
                "--write",
            ],
        ),
        MigrationCommand::Freeze => run_process(
            &root_dir(),
            "python",
            &["scripts/check_migration_history.py", "--freeze"],
        ),
        MigrationCommand::Run { operation, target } => {
            let args = migration_run_args(*operation, target);
            run_owned(&root_dir(), "cargo", &args)
        }
        MigrationCommand::New { scope, name } => {
            let _lock = MigrationLock::acquire(&root_dir(), *scope)?;
            let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
            create_migration_under_lock(&root_dir(), *scope, name, &timestamp)
        }
    }
}

pub(crate) fn migration_run_args(
    operation: MigrationOperation,
    target: &MigrationTarget,
) -> Vec<String> {
    let mut args = [
        "run",
        "--locked",
        "-p",
        "ryframe",
        "--no-default-features",
        "--features",
        "bin-migrate",
        "--bin",
        "ryframe-migrate",
        "--",
    ]
    .map(str::to_owned)
    .to_vec();
    let operation = operation.as_str().to_owned();
    match target {
        MigrationTarget::Control => args.extend(["control".to_owned(), operation]),
        MigrationTarget::TenantDataAll => {
            args.extend(["tenant-data".to_owned(), operation, "--all".to_owned()]);
        }
        MigrationTarget::TenantDataOne(target) => args.extend([
            "tenant-data".to_owned(),
            operation,
            "--target".to_owned(),
            target.clone(),
        ]),
    }
    args
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
    let updated_module_source =
        insert_migration_name(&insert_module_declaration(&module_source, module)?, module)?;

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
    updated.insert_str(
        insert_at,
        &format!("#[cfg(feature = \"migration\")]{newline}{declaration}{newline}"),
    );
    Ok(updated)
}

fn insert_migration_name(source: &str, module: &str) -> Result<String> {
    const REGISTRY: &str = "const HANDWRITTEN_MIGRATION_NAMES: &[&str] = &[";
    let entry = format!("\"{module}\"");
    if source.contains(&entry) {
        return Err(format!("迁移名称已注册：{module}").into());
    }
    let content_start = source
        .find(REGISTRY)
        .map(|offset| offset + REGISTRY.len())
        .ok_or("迁移模块注册表缺少 HANDWRITTEN_MIGRATION_NAMES")?;
    let content_end = source[content_start..]
        .find("];")
        .map(|offset| content_start + offset)
        .ok_or("HANDWRITTEN_MIGRATION_NAMES 缺少结束括号")?;
    let mut entries = source[content_start..content_end]
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if entries
        .iter()
        .any(|entry| !(entry.starts_with('"') && entry.ends_with('"')))
    {
        return Err("HANDWRITTEN_MIGRATION_NAMES 只能包含迁移名称字面量".into());
    }
    entries.push(&entry);
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let registry = format!(
        "{newline}{}{newline}",
        entries
            .iter()
            .map(|entry| format!("    {entry},"))
            .collect::<Vec<_>>()
            .join(newline)
    );
    let mut updated = source.to_owned();
    updated.replace_range(content_start..content_end, &registry);
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
