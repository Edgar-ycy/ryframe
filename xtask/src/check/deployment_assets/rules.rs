use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use percent_encoding::percent_decode_str;

use super::regex;
use crate::Result;

pub(super) const REPOSITORY_BLOB_PREFIX: &str = "https://github.com/Edgar-ycy/ryframe/blob/main/";

pub(crate) fn toolchain_violations(channel: &str, minimum: &str, source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    if !regex(r"^1\.\d+\.\d+$").is_match(channel) {
        violations.push("Rust 工具链必须固定到稳定版的完整版本号".to_owned());
        return violations;
    }
    if minimum != channel.rsplit_once('.').map_or("", |value| value.0) {
        violations.push("workspace rust-version 必须与固定 Rust 工具链的主次版本一致".to_owned());
    }
    let image_version = captures_all(r"(?m)^ARG RUST_IMAGE=rust:([^\s-]+)-", source);
    if image_version != [channel] {
        violations.push("生产构建镜像的 Rust 版本必须与 rust-toolchain.toml 完全一致".to_owned());
    }
    violations
}

pub(crate) fn validate_runbook_url(root: &Path, url: &str) -> Result<Option<String>> {
    let Some(rest) = url.strip_prefix(REPOSITORY_BLOB_PREFIX) else {
        return Ok(Some("不是稳定的绝对仓库 URL".to_owned()));
    };
    let Some((path, fragment)) = rest.split_once('#') else {
        return Ok(Some("必须同时包含仓库文档路径和标题锚点".to_owned()));
    };
    if path.is_empty() || fragment.is_empty() {
        return Ok(Some("必须同时包含仓库文档路径和标题锚点".to_owned()));
    }
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    let absolute_root = std::path::absolute(root)?;
    let document = std::path::absolute(absolute_root.join(decoded.as_ref()))?;
    if !document.starts_with(&absolute_root) {
        return Ok(Some("文档路径越过仓库边界".to_owned()));
    }
    if !document.is_file() {
        return Ok(Some(format!("对应文档不存在: {path}")));
    }
    let canonical_root = fs::canonicalize(&absolute_root)?;
    let canonical_document = fs::canonicalize(&document)?;
    if !canonical_document.starts_with(&canonical_root) {
        return Ok(Some("文档路径越过仓库边界".to_owned()));
    }
    let decoded_fragment = percent_decode_str(fragment)
        .decode_utf8_lossy()
        .to_lowercase();
    if !markdown_anchors(&canonical_document)?.contains(&decoded_fragment) {
        return Ok(Some(format!("对应标题锚点不存在: {fragment}")));
    }
    Ok(None)
}

pub(crate) fn markdown_anchors(path: &Path) -> Result<BTreeSet<String>> {
    let source = fs::read_to_string(path)?;
    let mut anchors = BTreeSet::new();
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut fence: Option<char> = None;
    for line in source.lines() {
        if let Some(capture) = regex(r"^[ \t]*(`{3,}|~{3,})").captures(line) {
            let marker = capture
                .get(1)
                .and_then(|value| value.as_str().chars().next())
                .ok_or("Markdown 代码围栏缺少标记")?;
            match fence {
                None => fence = Some(marker),
                Some(current) if current == marker => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let Some(capture) = regex(r"^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$").captures(line) else {
            continue;
        };
        let heading = capture
            .get(1)
            .map(|value| value.as_str().trim().to_lowercase())
            .unwrap_or_default();
        let mut base = regex(r"[^\w\- ]").replace_all(&heading, "").into_owned();
        base = regex(r"[ \t]+").replace_all(&base, "-").into_owned();
        if base.is_empty() {
            continue;
        }
        let count = occurrences.entry(base.clone()).or_default();
        anchors.insert(if *count == 0 {
            base
        } else {
            format!("{base}-{count}")
        });
        *count += 1;
    }
    Ok(anchors)
}

fn captures_all<'a>(pattern: &str, source: &'a str) -> Vec<&'a str> {
    regex::Regex::new(pattern)
        .expect("调用方固定正则")
        .captures_iter(source)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
        .collect()
}
