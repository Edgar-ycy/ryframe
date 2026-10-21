use std::{fs, path::Path};

use super::markdown;

pub(super) fn english_path(chinese: &str) -> String {
    format!("{}.en.md", chinese.trim_end_matches(".md"))
}

/// 检查结构与技术标识；译文语义仍须结合实际实现人工审查。
pub(super) fn validate(root: &Path, chinese: &str, errors: &mut Vec<String>) {
    let english = english_path(chinese);
    let (Ok(chinese_source), Ok(english_source)) = (
        fs::read_to_string(root.join(chinese)),
        fs::read_to_string(root.join(&english)),
    ) else {
        return;
    };
    let left = markdown::parse(&chinese_source);
    let right = markdown::parse(&english_source);
    let same_levels = left
        .headings
        .iter()
        .map(|(level, _)| level)
        .eq(right.headings.iter().map(|(level, _)| level));
    if !same_levels || left.bullets != right.bullets {
        errors.push(format!(
            "中英文文档章节或条目数量不一致：{chinese} / {english}"
        ));
    }
    if left.blocks != right.blocks {
        errors.push(format!("中英文文档代码示例不一致：{chinese} / {english}"));
    }
    if markdown::inline_codes(&left.prose) != markdown::inline_codes(&right.prose) {
        errors.push(format!(
            "中英文文档命令、参数或行内技术标识不一致：{chinese} / {english}"
        ));
    }
    if chinese == "CHANGELOG.md" && markdown::history_keys(&left) != markdown::history_keys(&right)
    {
        errors.push("中英文 Changelog 的版本、日期或历史顺序不一致".into());
    }
    for (relative, document, counterpart) in [
        (chinese, &left, english.as_str()),
        (english.as_str(), &right, chinese),
    ] {
        if document
            .blocks
            .iter()
            .any(|(language, _)| language.starts_with("unclosed:"))
        {
            errors.push(format!("文档代码围栏未闭合：{relative}"));
        }
        let filename = Path::new(counterpart)
            .file_name()
            .unwrap()
            .to_string_lossy();
        let links = markdown::links(&document.prose);
        if !links.iter().any(|target| target == filename.as_ref()) {
            errors.push(format!(
                "文档缺少另一语言的相对切换链接：{relative} -> {filename}"
            ));
        }
        for target in links {
            validate_link(root, relative, &target, errors);
        }
    }
}

fn validate_link(root: &Path, relative: &str, target: &str, errors: &mut Vec<String>) {
    let target = target.trim_matches(['<', '>']);
    if target.contains("://") || target.starts_with("mailto:") {
        return;
    }
    let (file, fragment) = target
        .split_once('#')
        .map_or((target, None), |(file, anchor)| (file, Some(anchor)));
    let decoded = percent_encoding::percent_decode_str(file).decode_utf8_lossy();
    let path = if file.is_empty() {
        root.join(relative)
    } else {
        root.join(Path::new(relative).parent().unwrap())
            .join(decoded.as_ref())
    };
    if !path.exists() {
        errors.push(format!("文档本地引用不存在：{relative} -> {target}"));
        return;
    }
    if path.extension().is_some_and(|extension| extension == "md")
        && let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty())
        && let Ok(source) = fs::read_to_string(&path)
    {
        let fragment = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
        if !markdown::anchors(&markdown::parse(&source)).contains(fragment.as_ref()) {
            errors.push(format!("文档章节引用不存在：{relative} -> {target}"));
        }
    }
}
