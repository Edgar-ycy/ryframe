use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use super::{
    discover::{discover, discover_generated, source_files},
    git,
    model::{CONTROL_BASELINE, CONTROL_PREFIX, TENANT_BASELINE, relative_posix},
};

pub(crate) fn verify_append_only(
    root: &Path,
    locked_paths: Option<&BTreeSet<String>>,
    require_frozen: bool,
    head_paths: Option<&BTreeSet<String>>,
) -> Vec<String> {
    let migrations = match discover(root) {
        Ok(migrations) => migrations,
        Err(error) => return vec![error],
    };
    let names = migrations
        .iter()
        .map(|migration| migration.name.as_str())
        .collect::<BTreeSet<_>>();
    let enforce_lock = locked_paths.is_some();
    let empty = BTreeSet::new();
    let locked_paths = locked_paths.unwrap_or(&empty);
    let owned_head_paths;
    let head_paths = match head_paths {
        Some(paths) => paths,
        None => match git::head_paths(root, "HEAD") {
            Ok(paths) => {
                owned_head_paths = paths;
                &owned_head_paths
            }
            Err(error) => return vec![error],
        },
    };
    let mut errors = Vec::new();
    for baseline in [CONTROL_BASELINE, TENANT_BASELINE] {
        if !names.contains(baseline) {
            errors.push(format!("缺少冻结基线迁移：{baseline}"));
        }
    }
    let mut registry_orders = BTreeMap::<PathBuf, Vec<(String, usize)>>::new();
    let mut name_registry_orders = BTreeMap::<PathBuf, Vec<(String, usize)>>::new();
    for migration in migrations {
        let control = migration.source.starts_with(root.join(CONTROL_PREFIX));
        let baseline = if control {
            CONTROL_BASELINE
        } else {
            TENANT_BASELINE
        };
        if migration.name != baseline && migration.name.as_str() <= baseline {
            errors.push(format!("追加迁移时间必须晚于基线：{}", migration.name));
        }
        if !migration.source.is_file() {
            if let Ok(relative) = relative_posix(root, &migration.source) {
                errors.push(format!("迁移缺少入口文件：{relative}"));
            } else {
                errors.push(format!("迁移缺少入口文件：{}", migration.source.display()));
            }
            continue;
        }
        let paths = match source_files(root, &migration) {
            Ok(paths) => paths,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let mut relatives = Vec::new();
        for path in &paths {
            match relative_posix(root, path) {
                Ok(relative) => relatives.push(relative),
                Err(error) => errors.push(error),
            }
        }
        let any_frozen = relatives
            .iter()
            .any(|relative| locked_paths.contains(relative));
        for relative in &relatives {
            if enforce_lock && (migration.name == baseline || any_frozen) {
                if !locked_paths.contains(relative) {
                    errors.push(format!("冻结迁移新增了未冻结源码：{relative}"));
                }
            } else if enforce_lock && head_paths.contains(relative) {
                errors.push(format!(
                    "已提交迁移未进入冻结清单：{relative}；禁止用自动检查接受历史文件，请从新建迁移的工作树执行 `cargo xtask data migrate freeze`"
                ));
            } else if enforce_lock && require_frozen {
                errors.push(format!(
                    "待提交迁移尚未冻结：{relative}；请先运行 `cargo xtask data migrate freeze`"
                ));
            }
        }
        match fs::read_to_string(&migration.source) {
            Ok(source) if migration.name == baseline || rejects_down_with_roll_forward(&source) => {
            }
            Ok(_) => errors.push(format!(
                "{} 的 down 必须返回说明追加修复策略的 DbErr::Custom，且不得执行其他逻辑",
                migration.name
            )),
            Err(error) => errors.push(format!(
                "无法读取迁移 {}：{error}",
                migration.source.display()
            )),
        }
        verify_registration(
            &migration,
            &mut registry_orders,
            &mut name_registry_orders,
            &mut errors,
        );
    }
    verify_order(
        root,
        registry_orders,
        "Migrator 必须按迁移名称递增注册",
        &mut errors,
    );
    verify_order(
        root,
        name_registry_orders,
        "只读迁移名称必须递增注册",
        &mut errors,
    );
    verify_generated(
        root,
        enforce_lock,
        require_frozen,
        locked_paths,
        head_paths,
        &mut errors,
    );
    errors
}

fn verify_registration(
    migration: &super::model::Migration,
    registry_orders: &mut BTreeMap<PathBuf, Vec<(String, usize)>>,
    name_registry_orders: &mut BTreeMap<PathBuf, Vec<(String, usize)>>,
    errors: &mut Vec<String>,
) {
    match fs::read_to_string(&migration.module_registry) {
        Ok(source) => {
            if !source.contains(&format!("mod {};", migration.name)) {
                errors.push(format!("迁移未在模块中注册：{}", migration.name));
            }
            let marker = format!("\"{}\"", migration.name);
            match source.find(&marker) {
                Some(position) => name_registry_orders
                    .entry(migration.module_registry.clone())
                    .or_default()
                    .push((migration.name.clone(), position)),
                None => errors.push(format!("迁移未加入只读名称注册表：{}", migration.name)),
            }
        }
        Err(error) => errors.push(format!(
            "无法读取迁移模块注册表 {}：{error}",
            migration.module_registry.display()
        )),
    }
    match fs::read_to_string(&migration.migrator_registry) {
        Ok(source) => {
            let marker = format!("{}::Migration", migration.name);
            match source.find(&marker) {
                Some(position) => registry_orders
                    .entry(migration.migrator_registry.clone())
                    .or_default()
                    .push((migration.name.clone(), position)),
                None => errors.push(format!("迁移未加入 Migrator 顺序：{}", migration.name)),
            }
        }
        Err(error) => errors.push(format!(
            "无法读取 Migrator 注册表 {}：{error}",
            migration.migrator_registry.display()
        )),
    }
}

fn verify_order(
    root: &Path,
    registries: BTreeMap<PathBuf, Vec<(String, usize)>>,
    message: &str,
    errors: &mut Vec<String>,
) {
    for (registry, mut entries) in registries {
        entries.sort_by_key(|(_name, position)| *position);
        let names = entries
            .into_iter()
            .map(|(name, _position)| name)
            .collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort();
        if names != sorted {
            errors.push(format!(
                "{message}：{}",
                relative_posix(root, &registry).unwrap_or_else(|_| registry.display().to_string())
            ));
        }
    }
}

fn verify_generated(
    root: &Path,
    enforce_lock: bool,
    require_frozen: bool,
    locked_paths: &BTreeSet<String>,
    head_paths: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    let sources = match discover_generated(root) {
        Ok(sources) => sources,
        Err(error) => {
            errors.push(error);
            return;
        }
    };
    for source in sources {
        let relative = match relative_posix(root, &source) {
            Ok(relative) => relative,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if enforce_lock && head_paths.contains(&relative) && !locked_paths.contains(&relative) {
            errors.push(format!(
                "已提交迁移未进入冻结清单：{relative}；资源初始迁移必须在提交前执行 `cargo xtask data migrate freeze`"
            ));
        } else if enforce_lock && require_frozen && !locked_paths.contains(&relative) {
            errors.push(format!(
                "待提交迁移尚未冻结：{relative}；请先运行 `cargo xtask data migrate freeze`"
            ));
        }
        let source_text = match fs::read_to_string(&source) {
            Ok(source) => source,
            Err(error) => {
                errors.push(format!("无法读取 generated migration {relative}：{error}"));
                continue;
            }
        };
        if !source_text.contains("INITIAL_RESOURCE_MIGRATION") {
            errors.push(format!(
                "generated migration 缺少不可变初始迁移标记：{relative}"
            ));
        }
        let resource = source
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let aggregate = source
            .parent()
            .and_then(Path::parent)
            .unwrap_or(root)
            .join("mod.rs");
        match fs::read_to_string(aggregate) {
            Ok(source) if source.contains(&format!("\"m_resource_initial_{resource}\"")) => {}
            Ok(_) => errors.push(format!(
                "generated migration 未加入只读名称注册表：{relative}"
            )),
            Err(error) => errors.push(format!("无法读取 generated migration 聚合注册表：{error}")),
        }
        if !rejects_down_with_roll_forward(&source_text) {
            errors.push(format!(
                "generated migration 必须拒绝 down、说明追加修复且不得执行其他逻辑：{relative}"
            ));
        }
    }
}

fn rejects_down_with_roll_forward(source: &str) -> bool {
    let Some(body) = down_body(source) else {
        return false;
    };
    let normalized = body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let body = normalized.strip_prefix("return").unwrap_or(&normalized);
    let Some(rest) = body.strip_prefix("Err(DbErr::Custom(\"") else {
        return false;
    };
    let mut escaped = false;
    let mut message_end = None;
    for (index, character) in rest.char_indices() {
        if character == '"' && !escaped {
            message_end = Some(index);
            break;
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    let Some(message_end) = message_end else {
        return false;
    };
    let message = &rest[..message_end];
    let mut tail = &rest[message_end + 1..];
    let Some(remainder) = tail
        .strip_prefix(".into()))")
        .or_else(|| tail.strip_prefix(".into(),))"))
    else {
        return false;
    };
    tail = remainder.strip_suffix(';').unwrap_or(remainder);
    tail = tail.strip_suffix(',').unwrap_or(tail);
    message.contains("追加") && tail.is_empty()
}

fn down_body(source: &str) -> Option<&str> {
    let mut offset = 0;
    while let Some(found) = source[offset..].find("async") {
        let start = offset + found;
        let after_async = start + "async".len();
        let mut cursor = skip_whitespace(source, after_async);
        if cursor == after_async {
            offset = after_async;
            continue;
        }
        if !source[cursor..].starts_with("fn")
            || source
                .as_bytes()
                .get(cursor + 2)
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            offset = after_async;
            continue;
        }
        let after_fn = cursor + 2;
        cursor = skip_whitespace(source, after_fn);
        if cursor == after_fn {
            offset = after_async;
            continue;
        }
        if !source[cursor..].starts_with("down")
            || source
                .as_bytes()
                .get(cursor + 4)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            offset = start + "async".len();
            continue;
        }
        cursor += 4;
        while let Some(character) = source[cursor..].chars().next() {
            match character {
                '{' => {
                    let body_start = cursor + 1;
                    let body_end = source[body_start..].find('}')? + body_start;
                    return Some(&source[body_start..body_end]);
                }
                '}' => break,
                _ => cursor += character.len_utf8(),
            }
        }
        offset = start + "async".len();
    }
    None
}

fn skip_whitespace(source: &str, mut cursor: usize) -> usize {
    while let Some(character) = source[cursor..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}
