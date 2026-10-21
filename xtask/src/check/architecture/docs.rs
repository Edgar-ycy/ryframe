use std::{collections::BTreeSet, fs, path::Path, sync::OnceLock};

use regex::Regex;

use super::support::{display_relative, read_text};

#[path = "docs/bilingual.rs"]
mod bilingual;
#[path = "docs/markdown.rs"]
mod markdown;

const DOCUMENT_LIMITS: &[(&str, usize)] = &[
    ("README.md", 120),
    ("docs/api.md", 180),
    ("docs/architecture.md", 200),
    ("docs/data.md", 200),
    ("docs/development.md", 160),
    ("docs/operations.md", 240),
];
const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".cache",
    ".git",
    ".github",
    ".local-tests",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
    "node_modules",
    "target",
    "vendor",
];
const SYSTEM_DOMAINS: &[&str] = &["content", "identity", "operations", "platform"];

pub(crate) fn run(root: &Path) -> crate::Result<()> {
    let mut errors = Vec::new();
    validate_documentation(root, &mut errors);
    if !errors.is_empty() {
        return Err(format!("中英文文档检查失败：\n{}", errors.join("\n")).into());
    }
    println!("后端中英文文档检查通过");
    Ok(())
}

pub(crate) fn validate_documentation(root: &Path, errors: &mut Vec<String>) {
    let actual = documentation_paths(root, errors);
    let expected = DOCUMENT_LIMITS
        .iter()
        .flat_map(|(path, _)| [(*path).to_owned(), bilingual::english_path(path)])
        .chain(["CHANGELOG.md".to_owned(), "CHANGELOG.en.md".to_owned()])
        .collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
        if !missing.is_empty() {
            errors.push(format!("后端缺少约定文档: {}", missing.join(", ")));
        }
        if !unexpected.is_empty() {
            errors.push(format!("后端存在额外人工文档: {}", unexpected.join(", ")));
        }
    }
    for (relative, limit) in DOCUMENT_LIMITS {
        for relative in [(*relative).to_owned(), bilingual::english_path(relative)] {
            let path = root.join(&relative);
            if path.is_file() && line_count(&path) > *limit {
                errors.push(format!(
                    "{relative} 共 {} 行，超过 {limit} 行上限",
                    line_count(&path)
                ));
            }
        }
    }
    for relative in actual.iter().filter(|path| !path.ends_with(".en.md")) {
        bilingual::validate(root, relative, errors);
    }
}

fn documentation_paths(root: &Path, errors: &mut Vec<String>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    if let Err(error) = walk_documents(root, root, &mut result) {
        errors.push(error);
    }
    result
}

fn walk_documents(
    root: &Path,
    directory: &Path,
    result: &mut BTreeSet<String>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| format!("无法枚举文档目录: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法枚举文档目录: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取文档类型 {}：{error}", display_relative(&path)))?;
        if file_type.is_dir() {
            let excluded = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let lowered = name.to_ascii_lowercase();
                    EXCLUDED_DIRECTORIES.contains(&lowered.as_str())
                });
            if !excluded {
                walk_documents(root, &path, result)?;
            }
        } else if file_type.is_file() && has_markdown_extension(&path) {
            result.insert(relative_path(root, &path));
        }
    }
    Ok(())
}

fn has_markdown_extension(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".md"))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|content| content.trim_end_matches(['\r', '\n']).lines().count())
        .unwrap_or(0)
}

pub(crate) fn validate_system_domain_surface(root: &Path, errors: &mut Vec<String>) {
    let path = root.join("crates/ryframe-application/src/system/mod.rs");
    if !path.is_file() {
        errors.push("缺少 ryframe-application system 模块入口".into());
        return;
    }
    let source = read_text(&path, " system 模块入口", errors);
    let public_modules = public_modules(&source);
    let expected = SYSTEM_DOMAINS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if public_modules != expected {
        errors.push(format!(
            "system 公开模块必须且只能为四个业务域: expected={:?}, actual={:?}",
            expected.iter().collect::<Vec<_>>(),
            public_modules.iter().collect::<Vec<_>>()
        ));
    }
    if public_use().is_match(&source) {
        errors.push("system 根模块不得保留兼容 re-export".into());
    }
}

fn public_modules(source: &str) -> BTreeSet<String> {
    public_module()
        .captures_iter(source)
        .filter_map(|capture| capture.get(1).map(|name| name.as_str().to_owned()))
        .collect()
}

fn public_module() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?m)^\s*pub\s+mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;").expect("固定正则必须有效")
    })
}

fn public_use() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?m)^\s*pub\s+use\s+").expect("固定正则必须有效"))
}
