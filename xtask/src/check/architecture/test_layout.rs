use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use regex::Regex;
use toml::{Table, Value};

use super::{
    support::{collect_files, is_rust_file, read_text, relative_posix},
    workspace::Packages,
};

pub(crate) fn validate_test_layout(
    root: &Path,
    raw: &Table,
    packages: &Packages,
    errors: &mut Vec<String>,
) -> (usize, usize) {
    let directory = validate_fixed_values(raw, errors);
    let max_lines = match raw
        .get("max_integration_test_lines")
        .and_then(Value::as_integer)
    {
        Some(value) if value > 0 => value as usize,
        _ => {
            errors.push("test_layout.max_integration_test_lines 必须是正整数".into());
            1
        }
    };
    let disabled = string_set(
        raw.get("disabled_bin_test_targets"),
        "test_layout.disabled_bin_test_targets",
        errors,
    );
    let actual_bins = packages
        .iter()
        .flat_map(|(name, package)| {
            package
                .targets
                .iter()
                .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
                .filter_map(move |target| {
                    target
                        .name
                        .as_ref()
                        .map(|target_name| format!("{name}:{target_name}"))
                })
        })
        .collect::<BTreeSet<_>>();
    for target in disabled.difference(&actual_bins) {
        errors.push(format!(
            "空测试 harness 策略引用不存在的二进制目标: {target}"
        ));
    }
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut checked_sources = 0;
    let mut integration_targets = 0;
    for (name, package) in packages {
        let Some(package_root) = repository_package_root(&canonical_root, package, name, errors)
        else {
            continue;
        };
        let test_root = package_root.join(&directory);
        if test_root.is_dir() {
            check_integration_files(&canonical_root, &test_root, max_lines, errors);
        }
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "bin") {
                check_binary_target(
                    &canonical_root,
                    &package_root,
                    package,
                    target,
                    &disabled,
                    errors,
                );
            }
            if target.kind.iter().any(|kind| kind == "test") {
                integration_targets += 1;
                let target_path = target
                    .src_path
                    .canonicalize()
                    .unwrap_or_else(|_| target.src_path.clone());
                let canonical_test_root = test_root
                    .canonicalize()
                    .unwrap_or_else(|_| test_root.clone());
                if !target_path.starts_with(&canonical_test_root) {
                    errors.push(format!(
                        "{name} 的测试目标必须位于本 crate/tests: {}",
                        relative_posix(&canonical_root, &target_path)
                    ));
                }
            }
        }
        let source_root = package_root.join("src");
        if source_root.is_dir() {
            match collect_files(&canonical_root, &source_root, true, is_rust_file) {
                Ok(files) => {
                    for relative in files {
                        checked_sources += 1;
                        let file_name = relative
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("");
                        if test_file_name().is_match(file_name) {
                            errors.push(format!(
                                "源码内单测必须与私有实现同文件，独立测试文件移至所属 crate/tests: {}",
                                relative.to_string_lossy().replace('\\', "/")
                            ));
                        }
                    }
                }
                Err(error) => errors.push(error),
            }
        }
    }
    (checked_sources, integration_targets)
}

fn validate_fixed_values(raw: &Table, errors: &mut Vec<String>) -> String {
    let directory = raw
        .get("directory")
        .and_then(Value::as_str)
        .filter(|value| *value == "tests")
        .unwrap_or("tests");
    if raw.get("directory").and_then(Value::as_str) != Some("tests") {
        errors.push("test_layout.directory 必须固定为 tests".into());
    }
    if raw
        .get("allow_colocated_unit_tests")
        .and_then(Value::as_bool)
        != Some(true)
    {
        errors.push("test_layout.allow_colocated_unit_tests 必须为 true".into());
    }
    if raw.get("forbid_source_test_files").and_then(Value::as_bool) != Some(true) {
        errors.push("test_layout.forbid_source_test_files 必须为 true".into());
    }
    directory.to_owned()
}

fn string_set(value: Option<&Value>, label: &str, errors: &mut Vec<String>) -> BTreeSet<String> {
    let Some(Value::Array(values)) = value else {
        errors.push(format!("{label} 必须是字符串数组"));
        return BTreeSet::new();
    };
    let mut result = BTreeSet::new();
    for value in values {
        let Some(value) = value.as_str() else {
            errors.push(format!("{label} 必须是字符串数组"));
            return BTreeSet::new();
        };
        if !result.insert(value.to_owned()) {
            errors.push(format!("{label} 不得包含重复项"));
        }
    }
    result
}

fn repository_package_root(
    root: &Path,
    package: &super::workspace::Package,
    name: &str,
    errors: &mut Vec<String>,
) -> Option<PathBuf> {
    let package_root = package
        .manifest_path
        .canonicalize()
        .ok()?
        .parent()?
        .to_path_buf();
    if !package_root.starts_with(root) {
        errors.push(format!("crate 位于仓库之外，无法检查测试位置: {name}"));
        return None;
    }
    Some(package_root)
}

fn check_integration_files(root: &Path, test_root: &Path, limit: usize, errors: &mut Vec<String>) {
    match collect_files(root, test_root, true, is_rust_file) {
        Ok(files) => {
            for relative in files {
                let lines = fs::read_to_string(root.join(&relative))
                    .map(|source| source.trim_end_matches(['\r', '\n']).lines().count())
                    .unwrap_or(0);
                if lines > limit {
                    errors.push(format!(
                        "集成测试文件超过 {limit} 行: {}（{lines} 行）",
                        relative.to_string_lossy().replace('\\', "/")
                    ));
                }
            }
        }
        Err(error) => errors.push(error),
    }
}

fn check_binary_target(
    root: &Path,
    package_root: &Path,
    package: &super::workspace::Package,
    target: &super::workspace::Target,
    disabled: &BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    let name = target.name.as_deref().unwrap_or("");
    let key = format!("{}:{name}", package.name);
    let test_disabled = target.test == Some(false);
    if test_disabled && !disabled.contains(&key) {
        errors.push(format!("关闭测试 harness 的二进制目标必须登记: {key}"));
    }
    if !disabled.contains(&key) {
        return;
    }
    if !test_disabled {
        errors.push(format!("二进制目标必须设置 test = false: {key}"));
    }
    let target_path = target
        .src_path
        .canonicalize()
        .unwrap_or_else(|_| target.src_path.clone());
    let mut sources = BTreeSet::from([target_path.clone()]);
    let companion_root = target_path.with_extension("");
    if companion_root.is_dir()
        && let Ok(files) = collect_files(root, &companion_root, true, is_rust_file)
    {
        sources.extend(files.into_iter().map(|path| root_for_file(root, path)));
    }
    if !package
        .targets
        .iter()
        .any(|item| item.kind.iter().any(|kind| kind == "lib"))
        && let Ok(files) = collect_files(root, &package_root.join("src"), true, is_rust_file)
    {
        sources.extend(files.into_iter().map(|path| root_for_file(root, path)));
    }
    for source_path in sources {
        let source = read_text(&source_path, "二进制源码", errors);
        if test_attribute().is_match(&source) {
            errors.push(format!(
                "已关闭测试 harness 的二进制源码不得包含单测: {}",
                relative_posix(root, &source_path)
            ));
        }
    }
}

fn root_for_file(root: &Path, relative: PathBuf) -> PathBuf {
    root.join(relative)
}

fn test_file_name() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(r"(?i)(?:^tests?\.rs$|_tests?\.rs$)").expect("固定正则必须有效"))
}

fn test_attribute() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|(?:[A-Za-z_][A-Za-z0-9_]*::)?test)\s*\]")
            .expect("固定正则必须有效")
    })
}
