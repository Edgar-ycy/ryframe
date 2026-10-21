use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

pub(super) struct Document {
    pub(super) prose: String,
    pub(super) blocks: Vec<(String, String)>,
    pub(super) headings: Vec<(usize, String)>,
    pub(super) bullets: usize,
}

pub(super) fn parse(source: &str) -> Document {
    let mut document = Document {
        prose: String::new(),
        blocks: Vec::new(),
        headings: Vec::new(),
        bullets: 0,
    };
    let mut fence: Option<(char, usize, String, String)> = None;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some((marker, length, language, body)) = &mut fence {
            if trimmed.chars().take_while(|value| value == marker).count() >= *length
                && trimmed.trim_matches(*marker).trim().is_empty()
            {
                document.blocks.push((language.clone(), body.clone()));
                fence = None;
            } else {
                let shell = matches!(
                    language.as_str(),
                    "powershell" | "pwsh" | "sh" | "bash" | "shell"
                );
                if !shell || !trimmed.starts_with('#') {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        } else if let Some(marker @ ('`' | '~')) = trimmed.chars().next() {
            let length = trimmed.chars().take_while(|value| *value == marker).count();
            if length >= 3 {
                fence = Some((
                    marker,
                    length,
                    trimmed[length..].trim().to_lowercase(),
                    String::new(),
                ));
            } else {
                document.prose.push_str(line);
                document.prose.push('\n');
            }
        } else {
            let level = trimmed.chars().take_while(|value| *value == '#').count();
            if (1..=6).contains(&level) && trimmed[level..].starts_with(' ') {
                document
                    .headings
                    .push((level, trimmed[level..].trim().to_owned()));
            }
            document.bullets += usize::from(trimmed.starts_with("- ") || trimmed.starts_with("* "));
            document.prose.push_str(line);
            document.prose.push('\n');
        }
    }
    if let Some((_, _, language, body)) = fence {
        document.blocks.push((format!("unclosed:{language}"), body));
    }
    document
}

pub(super) fn inline_codes(source: &str) -> BTreeMap<String, usize> {
    let pattern = Regex::new(r"`([^`\n]+)`").expect("固定行内代码表达式");
    let placeholders = Regex::new(r"<[^>\n]+>").expect("固定占位符表达式");
    let mut codes = BTreeMap::new();
    for capture in pattern.captures_iter(source) {
        let code = placeholders.replace_all(&capture[1], |value: &regex::Captures<'_>| {
            let inner = &value[0][1..value[0].len() - 1];
            if inner.chars().next().is_some_and(char::is_uppercase) {
                value[0].to_owned()
            } else {
                "<placeholder>".to_owned()
            }
        });
        *codes.entry(code.into_owned()).or_default() += 1;
    }
    codes
}

pub(super) fn links(source: &str) -> Vec<String> {
    let inline = Regex::new(r"`[^`\n]+`").expect("固定行内代码表达式");
    let prose = inline.replace_all(source, "");
    let pattern = Regex::new(r"\[[^\]\n]*\]\(([^\)\n]+)\)").expect("固定链接表达式");
    pattern
        .captures_iter(&prose)
        .map(|capture| capture[1].trim().to_owned())
        .collect()
}

pub(super) fn anchors(document: &Document) -> BTreeSet<String> {
    let mut seen = BTreeMap::<String, usize>::new();
    let mut result = BTreeSet::new();
    for (_, heading) in &document.headings {
        let slug = heading
            .chars()
            .filter(|character| character.is_alphanumeric() || matches!(character, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .replace(' ', "-");
        let count = seen.entry(slug.clone()).or_default();
        result.insert(if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    result
}

pub(super) fn history_keys(document: &Document) -> Vec<Vec<String>> {
    let pattern = Regex::new(r"v?\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?|\d{4}-\d{2}-\d{2}|Unreleased")
        .expect("固定版本日期表达式");
    document
        .headings
        .iter()
        .filter(|(level, _)| *level == 2)
        .map(|(_, heading)| {
            pattern
                .find_iter(heading)
                .map(|value| value.as_str().to_owned())
                .collect()
        })
        .collect()
}
