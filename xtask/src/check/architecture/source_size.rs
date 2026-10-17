use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use toml::{Table, Value};

use super::{
    policy::Profile,
    support::{collect_files, is_rust_file, read_text, relative_posix},
    workspace::{Packages, report_outside_repository},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SizeLevel {
    Hint,
    Strong,
    Fail,
}

pub(super) struct SourceStats {
    pub(super) scanned: usize,
    pub(super) by_package: BTreeMap<String, usize>,
    pub(super) frozen_migrations: BTreeSet<String>,
}

struct SourcePolicy {
    max_lines: usize,
    generated_max_lines: usize,
    generated_markers: Vec<String>,
    frozen_lock: Option<String>,
}

pub(super) fn validate_source_size(
    root: &Path,
    raw: &Table,
    profile: &Profile,
    packages: &Packages,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> SourceStats {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root = canonical_root.as_path();
    let Some(policy) = parse_policy(raw, errors) else {
        return empty_stats();
    };
    if raw.contains_key("legacy_exceptions") {
        errors.push("source_size 不再支持 legacy_exceptions".into());
    }
    let frozen_migrations = policy
        .frozen_lock
        .as_deref()
        .map(|relative| frozen_migration_sources(root, relative, errors))
        .unwrap_or_default();
    let mut scanned = BTreeSet::new();
    let mut by_package = BTreeMap::new();
    for name in profile.products.union(&profile.tools) {
        let Some(package) = packages.get(name) else {
            continue;
        };
        let Some(package_root) = repository_root(root, package, name, errors) else {
            continue;
        };
        let candidates = package_candidates(root, &package_root, errors);
        let mut count = 0;
        for relative in candidates {
            let path = root.join(&relative);
            scanned.insert(relative.clone());
            count += 1;
            if frozen_migrations.contains(&relative) {
                continue;
            }
            let lines = count_lines(&path);
            let limit = if has_generated_marker(&path, &policy.generated_markers) {
                policy.generated_max_lines
            } else {
                policy.max_lines
            };
            match source_size_level(lines, limit) {
                Some(SizeLevel::Fail) => errors.push(format!(
                    "源码文件达到 {limit} 行硬上限: {relative}（{lines} 行）"
                )),
                Some(SizeLevel::Strong) => warnings.push(format!(
                    "强警告：{relative} 已达硬上限 90%（{lines}/{limit} 行）"
                )),
                Some(SizeLevel::Hint) => warnings.push(format!(
                    "提示：{relative} 已达硬上限 80%（{lines}/{limit} 行）"
                )),
                None => {}
            }
        }
        if count == 0 {
            errors.push(format!("工作区 crate 没有纳入任何 Rust 源文件: {name}"));
        }
        by_package.insert(name.clone(), count);
    }
    for path in frozen_migrations.difference(&scanned) {
        errors.push(format!("冻结迁移清单未命中工作区源码: {path}"));
    }
    SourceStats {
        scanned: scanned.len(),
        by_package,
        frozen_migrations,
    }
}

fn parse_policy(raw: &Table, errors: &mut Vec<String>) -> Option<SourcePolicy> {
    let max_lines = positive_integer(raw, "max_lines", "source_size", errors)?;
    let generated_max_lines = positive_integer(raw, "generated_max_lines", "source_size", errors)?;
    if generated_max_lines > max_lines {
        errors.push("source_size.generated_max_lines 不得大于 source_size.max_lines".into());
    }
    let generated_markers = match raw.get("generated_markers").and_then(Value::as_array) {
        Some(values) if !values.is_empty() => {
            let parsed = values
                .iter()
                .map(|value| value.as_str().filter(|value| !value.trim().is_empty()))
                .collect::<Option<Vec<_>>>();
            match parsed {
                Some(values) => values.into_iter().map(str::to_owned).collect(),
                None => {
                    errors.push("source_size.generated_markers 只能包含非空字符串".into());
                    Vec::new()
                }
            }
        }
        _ => {
            errors.push("source_size.generated_markers 必须是非空字符串数组".into());
            Vec::new()
        }
    };
    let frozen_lock = raw
        .get("frozen_migration_lock")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if frozen_lock.is_none() {
        errors.push("source_size.frozen_migration_lock 必须是非空字符串".into());
    }
    Some(SourcePolicy {
        max_lines,
        generated_max_lines,
        generated_markers,
        frozen_lock,
    })
}

fn positive_integer(
    table: &Table,
    key: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) -> Option<usize> {
    match table.get(key).and_then(Value::as_integer) {
        Some(value) if value > 0 => Some(value as usize),
        _ => {
            errors.push(format!("{prefix}.{key} 必须是正整数"));
            None
        }
    }
}

fn package_candidates(root: &Path, package_root: &Path, errors: &mut Vec<String>) -> Vec<String> {
    let mut result = BTreeSet::new();
    let source_root = package_root.join("src");
    if source_root.is_dir() {
        match collect_files(root, &source_root, true, is_rust_file) {
            Ok(files) => result.extend(files.into_iter().map(|path| to_posix(&path))),
            Err(error) => errors.push(error),
        }
    }
    let build_script = package_root.join("build.rs");
    if build_script.is_file() {
        result.insert(relative_posix(root, &build_script));
    }
    result.into_iter().collect()
}

fn repository_root(
    root: &Path,
    package: &super::workspace::Package,
    name: &str,
    errors: &mut Vec<String>,
) -> Option<PathBuf> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let package_root = package
        .manifest_path
        .canonicalize()
        .ok()?
        .parent()?
        .to_path_buf();
    if !package_root.starts_with(&canonical_root) {
        report_outside_repository(name, &package_root, errors);
        return None;
    }
    Some(package_root)
}

fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn count_lines(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|content| content.trim_end_matches(['\r', '\n']).lines().count())
        .unwrap_or(0)
}

fn has_generated_marker(path: &Path, markers: &[String]) -> bool {
    let header = fs::read_to_string(path)
        .map(|source| source.lines().take(5).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    markers.iter().any(|marker| header.contains(marker))
}

pub(crate) fn source_size_level(lines: usize, limit: usize) -> Option<SizeLevel> {
    if lines >= limit {
        Some(SizeLevel::Fail)
    } else if lines * 10 >= limit * 9 {
        Some(SizeLevel::Strong)
    } else if lines * 5 >= limit * 4 {
        Some(SizeLevel::Hint)
    } else {
        None
    }
}

fn empty_stats() -> SourceStats {
    SourceStats {
        scanned: 0,
        by_package: BTreeMap::new(),
        frozen_migrations: BTreeSet::new(),
    }
}

pub(crate) fn frozen_migration_sources(
    root: &Path,
    relative: &str,
    errors: &mut Vec<String>,
) -> BTreeSet<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let lock_path = root.join(relative);
    let Ok(lock_path) = lock_path.canonicalize() else {
        errors.push(format!("无法读取冻结迁移清单 {relative}"));
        return BTreeSet::new();
    };
    if !lock_path.starts_with(&canonical_root) {
        errors.push(format!("冻结迁移清单路径越出仓库: {relative}"));
        return BTreeSet::new();
    }
    let source = read_text(&lock_path, "冻结迁移清单", errors);
    let Ok(Value::Table(document)) = toml::from_str::<Value>(&source) else {
        errors.push(format!("无法读取冻结迁移清单 {relative}: TOML 解析失败"));
        return BTreeSet::new();
    };
    if document.get("format_version").and_then(Value::as_integer) != Some(1) {
        errors.push(format!("冻结迁移清单 format_version 必须为 1: {relative}"));
    }
    let Some(Value::Array(entries)) = document.get("files") else {
        errors.push(format!(
            "冻结迁移清单 files 必须是非空 table 数组: {relative}"
        ));
        return BTreeSet::new();
    };
    if entries.is_empty() {
        errors.push(format!(
            "冻结迁移清单 files 必须是非空 table 数组: {relative}"
        ));
        return BTreeSet::new();
    }
    let mut frozen = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = format!("冻结迁移清单 files[{index}]");
        let Some(entry) = entry.as_table() else {
            errors.push(format!("{label} 必须是 TOML table"));
            continue;
        };
        let Some(path) = entry
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            errors.push(format!("{label}.path 必须是非空字符串"));
            continue;
        };
        let Some(expected_hash) = entry.get("sha256").and_then(Value::as_str) else {
            errors.push(format!("{label}.sha256 必须是 64 位小写十六进制摘要"));
            continue;
        };
        if !valid_sha256(expected_hash) {
            errors.push(format!("{label}.sha256 必须是 64 位小写十六进制摘要"));
            continue;
        }
        let source_path = root.join(path);
        let Ok(canonical_source) = source_path.canonicalize() else {
            errors.push(format!("冻结迁移源码不存在: {path}"));
            continue;
        };
        if !canonical_source.starts_with(&canonical_root) {
            errors.push(format!("{label}.path 越出仓库: {path}"));
            continue;
        }
        let relative_path = canonical_source
            .strip_prefix(&canonical_root)
            .unwrap_or(&canonical_source)
            .to_string_lossy()
            .replace('\\', "/");
        if !relative_path.starts_with("crates/")
            || !relative_path.contains("/migration/")
            || !relative_path.ends_with(".rs")
        {
            errors.push(format!(
                "{label}.path 必须是 crates 下的迁移 Rust 源码: {relative_path}"
            ));
            continue;
        }
        if !frozen.insert(relative_path.clone()) {
            errors.push(format!("冻结迁移清单路径重复: {relative_path}"));
            continue;
        }
        let actual_hash = sha256_hex(&fs::read(&canonical_source).unwrap_or_default());
        if actual_hash != expected_hash {
            errors.push(format!(
                "冻结迁移源码哈希不匹配: {relative_path}（清单 {expected_hash}，实际 {actual_hash}）"
            ));
            frozen.remove(&relative_path);
        }
    }
    frozen
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
