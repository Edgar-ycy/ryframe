use std::{collections::BTreeMap, env, ffi::OsStr, path::Path, sync::OnceLock};

use regex::Regex;

use super::super::support::command_stdout;

pub(crate) fn changed_line_ranges(
    root: &Path,
    errors: &mut Vec<String>,
) -> Option<BTreeMap<String, Vec<(usize, usize)>>> {
    let configured = env::var("RYFRAME_CI_BASE_SHA")
        .ok()
        .or_else(|| env::var("GITHUB_BASE_SHA").ok());
    if configured
        .as_ref()
        .is_some_and(|base| !git_sha().is_match(base))
    {
        errors.push("函数长度 changed 模式收到无效 base SHA，已回退检查全部函数".into());
        return None;
    }
    let base = configured.as_deref().unwrap_or("HEAD");
    let diff = match command_stdout(
        root,
        OsStr::new("git"),
        &["diff", "--unified=0", "--no-ext-diff", base, "--", "*.rs"],
        None,
    ) {
        Ok(diff) => diff,
        Err(_) => {
            errors.push("函数长度 changed 模式无法读取 Git diff，已回退检查全部函数".into());
            return None;
        }
    };
    let mut changed = parse_changed_ranges(&diff);
    let untracked = match command_stdout(
        root,
        OsStr::new("git"),
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*.rs",
        ],
        None,
    ) {
        Ok(output) => output,
        Err(_) => {
            errors.push("函数长度 changed 模式无法读取未跟踪 Rust 文件，已回退检查全部函数".into());
            return None;
        }
    };
    for path in parse_untracked_rust_paths(&untracked) {
        changed.insert(path, vec![(1, i32::MAX as usize)]);
    }
    Some(changed)
}

fn git_sha() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)^[0-9a-f]{40}$").expect("固定正则必须有效"))
}

fn hunk_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^@@ -\d+(?:,\d+)? \+(?P<start>\d+)(?:,(?P<count>\d+))? @@")
            .expect("固定正则必须有效")
    })
}

pub(crate) fn parse_changed_ranges(diff: &str) -> BTreeMap<String, Vec<(usize, usize)>> {
    let mut result = BTreeMap::<String, Vec<(usize, usize)>>::new();
    let mut current = None::<String>;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            result.entry(path.to_owned()).or_default();
            current = Some(path.to_owned());
            continue;
        }
        let Some(current) = current.as_ref() else {
            continue;
        };
        let Some(captures) = hunk_pattern().captures(line) else {
            continue;
        };
        let Some(start) = captures
            .name("start")
            .and_then(|value| value.as_str().parse::<usize>().ok())
        else {
            continue;
        };
        let count = captures
            .name("count")
            .and_then(|value| value.as_str().parse::<usize>().ok())
            .unwrap_or(1);
        if count > 0 {
            result
                .entry(current.clone())
                .or_default()
                .push((start, start + count - 1));
        }
    }
    result
}

pub(crate) fn parse_untracked_rust_paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|path| !path.is_empty() && path.replace('\\', "/").ends_with(".rs"))
        .map(|path| path.replace('\\', "/"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
