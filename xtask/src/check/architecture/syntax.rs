use std::{collections::BTreeSet, path::Path, sync::OnceLock};

use regex::Regex;

use super::{
    policy::Profile,
    support::{collect_files, is_rust_file, read_text, relative_posix},
    workspace::Packages,
};

const LEGACY_PERSISTENCE_API_PARTS: &[(&str, &str)] =
    &[("Persistence", "Future"), ("Control", "Transaction")];

pub(super) fn workspace_rust_sources(
    root: &Path,
    profile: &Profile,
    packages: &Packages,
) -> Vec<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut sources = BTreeSet::new();
    for name in profile.products.union(&profile.tools) {
        let Some(package) = packages.get(name) else {
            continue;
        };
        let Some(package_root) = package
            .manifest_path
            .canonicalize()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            continue;
        };
        if !package_root.starts_with(&canonical_root) {
            continue;
        }
        for directory in ["src", "tests"] {
            let directory = package_root.join(directory);
            if directory.is_dir()
                && let Ok(files) = collect_files(&canonical_root, &directory, true, is_rust_file)
            {
                sources.extend(files.into_iter().map(|path| path_to_string(&path)));
            }
        }
        let build_script = package_root.join("build.rs");
        if build_script.is_file() {
            sources.insert(relative_posix(&canonical_root, &build_script));
        }
    }
    sources.into_iter().collect()
}

pub(super) fn workspace_rust_templates(
    root: &Path,
    profile: &Profile,
    packages: &Packages,
) -> Vec<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut templates = BTreeSet::new();
    for name in profile.products.union(&profile.tools) {
        let Some(package) = packages.get(name) else {
            continue;
        };
        let Some(package_root) = package
            .manifest_path
            .canonicalize()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            continue;
        };
        if !package_root.starts_with(&canonical_root) {
            continue;
        }
        if let Ok(files) = collect_files(&canonical_root, &package_root, true, |path| {
            path.to_string_lossy().ends_with(".rs.tpl")
        }) {
            templates.extend(files.into_iter().map(|path| path_to_string(&path)));
        }
    }
    templates.into_iter().collect()
}

pub(crate) fn validate_legacy_persistence_apis(
    root: &Path,
    sources: &[String],
    errors: &mut Vec<String>,
) -> (usize, usize) {
    let mut violations = 0;
    for relative in sources {
        let source = read_text(&root.join(relative), "工作区源码", errors);
        for (prefix, suffix) in LEGACY_PERSISTENCE_API_PARTS {
            let name = format!("{prefix}{suffix}");
            if contains_identifier(&source, &name) {
                errors.push(format!(
                    "工作区源码不得使用已删除的持久化接口 {name}: {relative}"
                ));
                violations += 1;
            }
        }
    }
    (sources.len(), violations)
}

pub(crate) fn validate_async_port_traits(
    root: &Path,
    sources: &[String],
    errors: &mut Vec<String>,
) -> (usize, usize) {
    let mut violations = 0;
    for relative in sources {
        let source = read_text(&root.join(relative), "工作区源码", errors);
        if !source.contains("Future") && !source.contains("Pin<") {
            continue;
        }
        for declaration in trait_declaration().captures_iter(&source) {
            let Some(name) = declaration.name("name").map(|capture| capture.as_str()) else {
                continue;
            };
            let whole = declaration.get(0).expect("trait 声明捕获组必须存在");
            let Some(body) = trait_body(&source, whole.end() - 1) else {
                continue;
            };
            for method in trait_future_method().captures_iter(body) {
                let method_name = method
                    .name("method")
                    .map(|capture| capture.as_str())
                    .unwrap_or("");
                let method_match = method.get(0).expect("trait 方法捕获组必须存在");
                let offset = whole.end() + method_match.start();
                let line = source[..offset]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                if method.name("async").is_some() {
                    errors.push(format!(
                        "异步端口 trait 不得从 async fn 返回手写 Future: {relative}:{line} {name}::{method_name}"
                    ));
                } else {
                    errors.push(format!(
                        "异步端口 trait 必须使用 async fn，而不得返回手写 Future: {relative}:{line} {name}::{method_name}"
                    ));
                }
                violations += 1;
            }
        }
    }
    (sources.len(), violations)
}

fn contains_identifier(source: &str, identifier: &str) -> bool {
    source.match_indices(identifier).any(|(index, _)| {
        let before = source[..index].chars().next_back();
        let after = source[index + identifier.len()..].chars().next();
        before.is_none_or(|character| !is_identifier(character))
            && after.is_none_or(|character| !is_identifier(character))
    })
}

fn is_identifier(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn trait_body(source: &str, opening_brace: usize) -> Option<&str> {
    let mut depth = 1;
    for (offset, character) in source[opening_brace + 1..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let start = opening_brace + 1;
                    return Some(&source[start..start + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

fn trait_declaration() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b[^{}]*\{",
        )
        .expect("固定正则必须有效")
    })
}

fn trait_future_method() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?s)\b(?P<async>async\s+)?fn\s+(?P<method>[A-Za-z_][A-Za-z0-9_]*)\b[^;{}]*?->\s*[^;{}]*(?:\b[A-Za-z_][A-Za-z0-9_]*Future\b|\bimpl\s+Future\b|\bPin\s*<)[^;{}]*;",
        )
        .expect("固定正则必须有效")
    })
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned()
}
