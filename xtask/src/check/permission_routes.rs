//! 校验访问目录编译门禁与已移除的在线生成器边界。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::Result;

struct Rule {
    label: &'static str,
    pattern: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        label: "在线生成器 handler",
        pattern: r"\bgenerator_handler\b",
    },
    Rule {
        label: "在线生成器 API 路径",
        pattern: r#"/api/v1/tools/gen(?:/|["'])"#,
    },
    Rule {
        label: "在线生成器路由段",
        pattern: r#"["']/gen["']"#,
    },
    Rule {
        label: "在线生成器权限",
        pattern: r"\btools:gen(?::[a-z]+)?\b",
    },
    Rule {
        label: "在线生成器菜单路由",
        pattern: r"\btools\.gen\b",
    },
];

fn compiled() -> &'static [(Rule, Regex)] {
    static COMPILED: OnceLock<Vec<(Rule, Regex)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .map(|rule| {
                let regex = Regex::new(rule.pattern).expect("固定权限路由正则");
                (Rule {
                    label: rule.label,
                    pattern: rule.pattern,
                }, regex)
            })
            .collect()
    })
}

fn source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let crates = root.join("crates");
    for entry in std::fs::read_dir(&crates)? {
        let entry = entry?;
        if !entry.path().is_dir() || entry.file_name().to_string_lossy() == "ryframe-generator" {
            continue;
        }
        let source = entry.path().join("src");
        if !source.is_dir() {
            continue;
        }
        let mut pending = vec![source];
        while let Some(directory) = pending.pop() {
            for child in std::fs::read_dir(&directory)? {
                let child = child?;
                if child.path().is_dir() {
                    pending.push(child.path());
                } else if child.path().extension().and_then(|name| name.to_str()) == Some("rs") {
                    files.push(child.path());
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn violations(root: &Path) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for path in source_files(root)? {
        let text = std::fs::read_to_string(&path)?;
        for (rule, regex) in compiled().iter() {
            if regex.is_match(&text) {
                result.push(format!("{} :: {}", path.display(), rule.label));
            }
        }
    }
    result.sort();
    Ok(result)
}

pub(crate) fn run(root: &Path) -> Result<()> {
    let violations = violations(root)?;
    if !violations.is_empty() {
        println!("访问边界违规：");
        for item in &violations {
            println!("  - {item}");
        }
        return Err(format!(
            "访问目录与路由 policy 检查发现 {} 处违规",
            violations.len()
        )
        .into());
    }
    crate::process::run_owned(root, "cargo", &["check".into(), "--locked".into(), "-p".into(), "ryframe-api".into()])?;
    println!("访问目录与路由 policy 检查通过。");
    Ok(())
}

pub(crate) fn run_scan_only(root: &Path) -> Result<()> {
    let violations = violations(root)?;
    if !violations.is_empty() {
        println!("访问边界违规：");
        for item in &violations {
            println!("  - {item}");
        }
        return Err(format!(
            "访问目录与路由 policy 检查发现 {} 处违规",
            violations.len()
        )
        .into());
    }
    Ok(())
}
