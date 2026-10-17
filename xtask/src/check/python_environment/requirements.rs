use std::collections::BTreeMap;

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Requirement {
    pub(crate) name: String,
    pub(crate) version: String,
}

pub(crate) fn parse(label: &str, source: &str) -> Result<Vec<Requirement>> {
    let mut requirements = Vec::new();
    let mut seen = BTreeMap::<String, usize>::new();
    for (line, logical) in logical_lines(label, source)? {
        let mut parts = logical.split_whitespace();
        let specification = parts
            .next()
            .ok_or_else(|| format!("{label}:{line} 为空要求行"))?;
        let (name, version) = specification.split_once("==").ok_or_else(|| {
            format!("{label}:{line} 必须使用严格 name==version，实际为 {specification}")
        })?;
        if !valid_distribution_name(name) {
            return Err(format!("{label}:{line} 发行版名称无效：{name}").into());
        }
        if !valid_exact_version(version) {
            return Err(
                format!("{label}:{line} 必须使用精确发行版版本，实际为 {specification}").into(),
            );
        }
        for option in parts {
            validate_hash_option(label, line, option)?;
        }
        let canonical = canonical_name(name);
        if let Some(previous) = seen.insert(canonical, line) {
            return Err(format!("{label}:{line} 与第 {previous} 行重复声明发行版 {name}").into());
        }
        requirements.push(Requirement {
            name: name.to_owned(),
            version: version.to_owned(),
        });
    }
    if requirements.is_empty() {
        return Err(format!("固定 Python 依赖为空：{label}").into());
    }
    Ok(requirements)
}

fn logical_lines(label: &str, source: &str) -> Result<Vec<(usize, String)>> {
    let mut rows = Vec::new();
    let mut pending = None::<(usize, String)>;
    for (index, physical) in source.lines().enumerate() {
        let line = index + 1;
        let trimmed = physical.trim();
        if pending.is_none() {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            pending = Some((line, String::new()));
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Err(format!("{label}:{line} 的续行不能为空或注释").into());
        }
        let continued = trimmed.ends_with('\\');
        let content = if continued {
            trimmed[..trimmed.len() - 1].trim_end()
        } else {
            trimmed
        };
        let (_, logical) = pending.as_mut().expect("续行状态已建立");
        if !logical.is_empty() && !content.is_empty() {
            logical.push(' ');
        }
        logical.push_str(content);
        if !continued {
            rows.push(pending.take().expect("续行结束应存在逻辑行"));
        }
    }
    if let Some((line, _)) = pending {
        return Err(format!("{label}:{line} 的续行在文件结尾未结束").into());
    }
    Ok(rows)
}

fn valid_distribution_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

fn valid_exact_version(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-' | b'!')
        })
}

fn validate_hash_option(label: &str, line: usize, option: &str) -> Result<()> {
    let Some(hash) = option.strip_prefix("--hash=sha256:") else {
        return Err(format!("{label}:{line} 包含未知 requirements 选项：{option}").into());
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label}:{line} 的 sha256 摘要必须为 64 位十六进制").into());
    }
    Ok(())
}

fn canonical_name(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'_' | b'.' => '-',
            other => other.to_ascii_lowercase() as char,
        })
        .collect()
}
