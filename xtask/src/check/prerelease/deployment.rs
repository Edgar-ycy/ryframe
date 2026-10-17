//! 精简部署标量解析；未解析引用及未归属字面量均失败关闭。

use std::collections::{BTreeMap, BTreeSet};

use super::{Finding, version};

pub(crate) fn yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if let Some(quote) = value.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        return value[1..]
            .split(quote)
            .next()
            .unwrap_or_default()
            .to_owned();
    }
    let end = value.char_indices().find_map(|(index, c)| {
        (c == '#' && index > 0 && value[..index].ends_with(char::is_whitespace)).then_some(index)
    });
    value[..end.unwrap_or(value.len())].trim().to_owned()
}

fn yaml_pair(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let line = line.strip_prefix('-').unwrap_or(line).trim_start();
    let (key, value) = line.split_once(':')?;
    let key = key.trim_end();
    (!value.trim().is_empty()).then_some((key, value.trim()))
}

fn variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn instruction<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let line = line.trim_start();
    let end = line.find(char::is_whitespace)?;
    (line[..end].eq_ignore_ascii_case(name))
        .then(|| line[end..].trim())
        .filter(|s| !s.is_empty())
}

fn lines(source: &str) -> impl Iterator<Item = &str> {
    source.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ])
}

fn deployment_key(key: &str) -> &str {
    let key = key.strip_prefix(['\'', '"']).unwrap_or(key);
    key.strip_suffix(['\'', '"']).unwrap_or(key)
}

pub(crate) fn variables(source: &str) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::new();
    for line in lines(source) {
        let docker = instruction(line, "ARG")
            .or_else(|| instruction(line, "ENV"))
            .and_then(|rest| {
                rest.char_indices()
                    .find(|(_, c)| *c == '=' || c.is_whitespace())
                    .map(|(i, c)| (&rest[..i], rest[i + c.len_utf8()..].trim()))
            })
            .filter(|(name, value)| variable_name(&name.to_ascii_uppercase()) && !value.is_empty());
        let pair = docker.or_else(|| yaml_pair(line).filter(|(name, _)| variable_name(name)));
        if let Some((name, value)) = pair {
            variables.insert(name.to_owned(), yaml_scalar(value));
        }
    }
    variables
}

fn substitute(source: &str, variables: &BTreeMap<String, String>, style: u8) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(offset) = source[cursor..].find('$') {
        let start = cursor + offset;
        output.push_str(&source[cursor..start]);
        let rest = &source[start..];
        let candidate = match style {
            0 => rest
                .strip_prefix("${{")
                .and_then(|s| s.find("}}").map(|end| (s[..end].trim(), end + 5)))
                .and_then(|(name, len)| name.strip_prefix("env.").map(|name| (name, None, len))),
            1 | 2 => rest
                .strip_prefix("${")
                .and_then(|s| s.find('}').map(|end| (&s[..end], end + 3)))
                .and_then(|(inner, len)| match (style, inner.split_once(":-")) {
                    (1, Some((name, default))) => Some((name, Some(default), len)),
                    (2, None) => Some((inner, None, len)),
                    _ => None,
                }),
            _ => {
                let len = rest[1..]
                    .bytes()
                    .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
                    .count();
                Some((&rest[1..1 + len], None, len + 1))
            }
        };
        if let Some((name, default, len)) = candidate.filter(|(name, _, _)| variable_name(name)) {
            output.push_str(
                variables
                    .get(name)
                    .map(String::as_str)
                    .or(default)
                    .unwrap_or(&rest[..len]),
            );
            cursor = start + len;
        } else {
            output.push('$');
            cursor = start + 1;
        }
    }
    output.push_str(&source[cursor..]);
    output
}

pub(crate) fn resolve(reference: &str, variables: &BTreeMap<String, String>) -> String {
    let mut resolved = reference.to_owned();
    for _ in 0..8 {
        let previous = resolved.clone();
        for style in 0..4 {
            resolved = substitute(&resolved, variables, style);
        }
        if resolved == previous {
            break;
        }
    }
    resolved
}

pub(crate) fn container(reference: &str) -> Option<(&str, &str)> {
    let reference = reference.trim().split('@').next()?;
    let colon = reference.rfind(':')?;
    if reference.rfind('/').is_some_and(|slash| colon <= slash) {
        return None;
    }
    let (name, version) = (&reference[..colon], &reference[colon + 1..]);
    (!name.is_empty() && !version.is_empty()).then_some((name, version))
}

fn unresolved(reference: &str) -> bool {
    reference.contains(['$', '\\', '`']) || reference.trim().starts_with(['*', '|', '>'])
}

pub(crate) fn docker_image(source: &str) -> Option<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut active = false;
    let mut quote = None;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '#') => break,
            (None, ' ' | '\t' | '\r' | '\n') => {
                if active {
                    tokens.push(std::mem::take(&mut token));
                    active = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                active = true;
            }
            (Some(q), c) if q == c => quote = None,
            (None | Some('"'), '\\') => {
                let next = chars.next()?;
                if quote.is_some() && !matches!(next, '"' | '\\') {
                    token.push('\\');
                }
                token.push(next);
                active = true;
            }
            _ => {
                token.push(c);
                active = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if active {
        tokens.push(token);
    }
    let mut index = 0;
    while tokens
        .get(index)
        .is_some_and(|token| token.starts_with("--"))
    {
        index += if tokens[index].contains('=') { 1 } else { 2 };
    }
    tokens.get(index).cloned()
}

fn add(findings: &mut BTreeSet<Finding>, counts: &mut BTreeMap<String, i64>, finding: Finding) {
    *counts.entry(finding.version.clone()).or_default() += 1;
    findings.insert(finding);
}

fn add_container(
    findings: &mut BTreeSet<Finding>,
    counts: &mut BTreeMap<String, i64>,
    reference: &str,
    path: &str,
) {
    if unresolved(reference) {
        add(
            findings,
            counts,
            Finding::new("container", reference, "unresolved", path),
        );
    } else if let Some((name, version)) = container(reference)
        && version::is_reference(version)
    {
        add(
            findings,
            counts,
            Finding::new("container", name, version, path),
        );
    }
}

pub(crate) fn findings(source: &str, path: &str) -> BTreeSet<Finding> {
    let mut findings = BTreeSet::new();
    let mut counts = BTreeMap::new();
    let vars = variables(source);
    let docker = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
        .starts_with("dockerfile");
    for line in lines(source) {
        let pair = yaml_pair(line).map(|(key, value)| (deployment_key(key), value));
        let image = if let Some(("image", value)) = pair {
            Some(yaml_scalar(value))
        } else if docker && let Some(value) = instruction(line, "FROM") {
            let image = docker_image(value);
            if image.is_none() {
                add(
                    &mut findings,
                    &mut counts,
                    Finding::new("container", value.trim(), "unresolved", path),
                );
            }
            image
        } else {
            None
        };
        if let Some(image) = image.filter(|image| !image.is_empty()) {
            add_container(&mut findings, &mut counts, &resolve(&image, &vars), path);
        }
        if let Some(("uses", value)) = pair {
            let action = resolve(&yaml_scalar(value), &vars);
            if let Some(reference) = action.strip_prefix("docker://") {
                add_container(&mut findings, &mut counts, reference, path);
            } else if unresolved(&action) {
                add(
                    &mut findings,
                    &mut counts,
                    Finding::new("tool", &action, "unresolved", path),
                );
            } else if let Some((name, version)) = action.rsplit_once('@')
                && !name.is_empty()
                && version::is_reference(version)
            {
                add(
                    &mut findings,
                    &mut counts,
                    Finding::new("tool", name, version, path),
                );
            }
        }
    }
    for version in version::literals(source) {
        *counts.entry(version).or_default() -= 1;
    }
    for (version, count) in counts {
        if count < 0 {
            findings.insert(Finding::new("deployment-literal", path, &version, path));
        }
    }
    findings
}
