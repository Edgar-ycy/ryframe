use std::collections::BTreeMap;

#[path = "yaml/scalar.rs"]
mod scalar;
use scalar::{parse_inline, parse_inline_with_anchors, strip_comment};

pub(crate) type YamlResult<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum YamlValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Sequence(Vec<YamlValue>),
    Mapping(Vec<(YamlKey, YamlValue)>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum YamlKey {
    String(String),
    Bool(bool),
    Integer(i64),
    Null,
}

impl YamlValue {
    pub(crate) fn as_mapping(&self) -> Option<&[(YamlKey, YamlValue)]> {
        match self {
            Self::Mapping(entries) => Some(entries),
            _ => None,
        }
    }

    pub(crate) fn as_sequence(&self) -> Option<&[YamlValue]> {
        match self {
            Self::Sequence(values) => Some(values),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&YamlValue> {
        self.as_mapping()?.iter().find_map(|(candidate, value)| {
            (candidate == &YamlKey::String(key.to_owned())).then_some(value)
        })
    }
}

pub(crate) fn parse(source: &str) -> YamlResult<YamlValue> {
    Parser::new(source)?.parse_document()
}

#[derive(Debug, Clone)]
struct Line<'a> {
    raw: &'a str,
    indent: usize,
    content: &'a str,
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    anchors: BTreeMap<String, YamlValue>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> YamlResult<Self> {
        let mut lines = Vec::new();
        for (index, raw) in source.lines().enumerate() {
            let indent = raw.bytes().take_while(|byte| *byte == b' ').count();
            if raw.as_bytes().get(indent) == Some(&b'\t') {
                return Err(format!(
                    "第 {} 行缩进包含制表符；YAML 缩进必须使用空格",
                    index + 1
                ));
            }
            lines.push(Line {
                raw,
                indent,
                content: &raw[indent..],
            });
        }
        Ok(Self {
            lines,
            anchors: BTreeMap::new(),
        })
    }

    fn parse_document(mut self) -> YamlResult<YamlValue> {
        let mut index = 0;
        self.skip_ignorable(&mut index);
        if index >= self.lines.len() {
            return Ok(YamlValue::Null);
        }
        let indent = self.lines[index].indent;
        if indent != 0 {
            return Err("工作流根节点不能使用缩进".into());
        }
        let value = self.parse_block(&mut index, indent)?;
        self.skip_ignorable(&mut index);
        if index < self.lines.len() {
            return Err(format!("第 {} 行存在无法归属的 YAML 内容", index + 1));
        }
        Ok(value)
    }

    fn parse_block(&mut self, index: &mut usize, indent: usize) -> YamlResult<YamlValue> {
        self.skip_ignorable(index);
        if *index >= self.lines.len() || self.lines[*index].indent < indent {
            return Ok(YamlValue::Null);
        }
        if self.lines[*index].indent > indent {
            return Err(format!("第 {} 行缩进无法归属", *index + 1));
        }
        if is_sequence_entry(self.lines[*index].content) {
            self.parse_sequence(index, indent)
        } else {
            self.parse_mapping(index, indent)
        }
    }

    fn parse_mapping(&mut self, index: &mut usize, indent: usize) -> YamlResult<YamlValue> {
        let mut entries = Vec::new();
        self.parse_mapping_entries(&mut entries, index, indent)?;
        self.finish_mapping(entries)
    }

    fn parse_mapping_entries(
        &mut self,
        entries: &mut Vec<(YamlKey, YamlValue)>,
        index: &mut usize,
        indent: usize,
    ) -> YamlResult<()> {
        loop {
            self.skip_ignorable(index);
            if *index >= self.lines.len() {
                break;
            }
            let line = &self.lines[*index];
            if line.indent < indent || is_sequence_entry(line.content) {
                break;
            }
            if line.indent > indent {
                return Err(format!("第 {} 行缩进无法归属", *index + 1));
            }
            let Some((raw_key, raw_value)) = split_mapping_entry(line.content) else {
                return Err(format!("第 {} 行不是有效的 YAML mapping 条目", *index + 1));
            };
            let key = parse_key(raw_key)?;
            *index += 1;
            let value = self.parse_mapping_value(raw_value, indent, index)?;
            insert_unique(entries, key, value)?;
        }
        Ok(())
    }

    fn parse_mapping_value(
        &mut self,
        raw_value: &str,
        parent_indent: usize,
        index: &mut usize,
    ) -> YamlResult<YamlValue> {
        let (anchor, remainder) = split_anchor(raw_value);
        let value = if remainder.trim().is_empty() {
            self.parse_nested_or_null(parent_indent, index)?
        } else if is_block_scalar(remainder) {
            self.parse_block_scalar(remainder, parent_indent, index)?
        } else {
            self.parse_inline_or_alias(remainder)?
        };
        self.register_anchor(anchor, &value)?;
        Ok(value)
    }

    fn parse_sequence(&mut self, index: &mut usize, indent: usize) -> YamlResult<YamlValue> {
        let mut values = Vec::new();
        loop {
            self.skip_ignorable(index);
            if *index >= self.lines.len() {
                break;
            }
            let line = &self.lines[*index];
            if line.indent < indent || line.indent == indent && !is_sequence_entry(line.content) {
                break;
            }
            if line.indent != indent {
                return Err(format!("第 {} 行缩进无法归属", *index + 1));
            }
            let after_dash = &line.content[1..];
            let mapping_indent = indent
                + 1
                + after_dash
                    .len()
                    .saturating_sub(after_dash.trim_start().len());
            let remainder = after_dash.trim_start();
            *index += 1;
            let (anchor, remainder) = split_anchor(remainder);
            let value = if remainder.is_empty() {
                self.parse_nested_or_null(indent, index)?
            } else if is_block_scalar(remainder) {
                self.parse_block_scalar(remainder, indent, index)?
            } else if let Some((raw_key, raw_value)) = split_mapping_entry(remainder) {
                let key = parse_key(raw_key)?;
                let value = self.parse_mapping_value(raw_value, mapping_indent, index)?;
                let mut entries = Vec::new();
                insert_unique(&mut entries, key, value)?;
                self.parse_mapping_entries(&mut entries, index, mapping_indent)?;
                self.finish_mapping(entries)?
            } else {
                self.parse_inline_or_alias(remainder)?
            };
            self.register_anchor(anchor, &value)?;
            values.push(value);
        }
        Ok(YamlValue::Sequence(values))
    }

    fn parse_nested_or_null(
        &mut self,
        parent_indent: usize,
        index: &mut usize,
    ) -> YamlResult<YamlValue> {
        self.skip_ignorable(index);
        if *index < self.lines.len() && self.lines[*index].indent > parent_indent {
            let indent = self.lines[*index].indent;
            self.parse_block(index, indent)
        } else {
            Ok(YamlValue::Null)
        }
    }

    fn parse_block_scalar(
        &mut self,
        header: &str,
        parent_indent: usize,
        index: &mut usize,
    ) -> YamlResult<YamlValue> {
        let header = strip_comment(header).trim();
        let folded = header.starts_with('>');
        let chomp = match header.chars().next_back() {
            Some('-') => '-',
            Some('+') => '+',
            _ => ' ',
        };
        let start = *index;
        let mut end = start;
        while end < self.lines.len() {
            let line = &self.lines[end];
            if !line.content.trim().is_empty() && line.indent <= parent_indent {
                break;
            }
            end += 1;
        }
        let content_indent = self.lines[start..end]
            .iter()
            .filter(|line| !line.content.trim().is_empty())
            .map(|line| line.indent)
            .min()
            .unwrap_or(parent_indent + 1);
        let mut content = self.lines[start..end]
            .iter()
            .map(|line| {
                if line.content.trim().is_empty() {
                    String::new()
                } else {
                    line.raw[content_indent.min(line.raw.len())..].to_owned()
                }
            })
            .collect::<Vec<_>>();
        while content.last().is_some_and(String::is_empty) {
            content.pop();
        }
        let mut text = if folded {
            fold_lines(&content)
        } else {
            content.join("\n")
        };
        match chomp {
            '-' => {
                while text.ends_with('\n') {
                    text.pop();
                }
            }
            '+' => {
                if !content.is_empty() {
                    text.push('\n');
                }
            }
            _ => {
                while text.ends_with('\n') {
                    text.pop();
                }
                if !content.is_empty() {
                    text.push('\n');
                }
            }
        }
        *index = end;
        Ok(YamlValue::String(text))
    }

    fn register_anchor(&mut self, anchor: Option<String>, value: &YamlValue) -> YamlResult<()> {
        if let Some(anchor) = anchor {
            self.anchors.insert(anchor, value.clone());
        }
        Ok(())
    }

    fn parse_inline_or_alias(&self, raw: &str) -> YamlResult<YamlValue> {
        let raw = strip_comment(raw).trim();
        if let Some(name) = raw.strip_prefix('*') {
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return Err("YAML alias 名称无效".into());
            }
            return self
                .anchors
                .get(name)
                .cloned()
                .ok_or_else(|| format!("未定义的 YAML alias：*{name}"));
        }
        parse_inline_with_anchors(raw, &self.anchors)
    }

    fn finish_mapping(&self, entries: Vec<(YamlKey, YamlValue)>) -> YamlResult<YamlValue> {
        let mut merged = Vec::new();
        for (key, value) in &entries {
            if key != &YamlKey::String("<<".to_owned()) {
                continue;
            }
            let values = match value {
                YamlValue::Mapping(_) => vec![value],
                YamlValue::Sequence(values) => values.iter().collect::<Vec<_>>(),
                _ => return Err("YAML merge key 的值必须是 mapping 或 mapping 数组".into()),
            };
            for source in values {
                let Some(source) = source.as_mapping() else {
                    return Err("YAML merge key 的值必须是 mapping 或 mapping 数组".into());
                };
                for (source_key, source_value) in source {
                    if merged
                        .iter()
                        .all(|(key, _): &(YamlKey, YamlValue)| key != source_key)
                    {
                        merged.push((source_key.clone(), source_value.clone()));
                    }
                }
            }
        }
        for (key, value) in entries {
            if key == YamlKey::String("<<".to_owned()) {
                continue;
            }
            if let Some(existing) = merged.iter_mut().find(|(candidate, _)| candidate == &key) {
                existing.1 = value;
            } else {
                merged.push((key, value));
            }
        }
        Ok(YamlValue::Mapping(merged))
    }

    fn skip_ignorable(&self, index: &mut usize) {
        while *index < self.lines.len() {
            let line = &self.lines[*index];
            let content = line.content.trim();
            if content.is_empty() || content.starts_with('#') || matches!(content, "---" | "...") {
                *index += 1;
            } else {
                break;
            }
        }
    }
}

fn split_mapping_entry(content: &str) -> Option<(&str, &str)> {
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0usize;
    for (index, character) in content.char_indices() {
        if let Some(current) = quote {
            if current == '"' && escaped {
                escaped = false;
            } else if current == '"' && character == '\\' {
                escaped = true;
            } else if character == current {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                let next = content[index + character.len_utf8()..].chars().next();
                if next.is_none_or(|next| next.is_whitespace() || next == '#') {
                    return Some((content[..index].trim(), content[index + 1..].trim_start()));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_key(raw: &str) -> YamlResult<YamlKey> {
    let value = parse_inline(raw)?;
    Ok(match value {
        YamlValue::String(value) => YamlKey::String(value),
        YamlValue::Bool(value) => YamlKey::Bool(value),
        YamlValue::Integer(value) => YamlKey::Integer(value),
        YamlValue::Null => YamlKey::Null,
        YamlValue::Float(_) | YamlValue::Sequence(_) | YamlValue::Mapping(_) => {
            return Err("YAML mapping 键必须是标量".into());
        }
    })
}

pub(super) fn insert_unique(
    entries: &mut Vec<(YamlKey, YamlValue)>,
    key: YamlKey,
    value: YamlValue,
) -> YamlResult<()> {
    if entries.iter().any(|(candidate, _)| candidate == &key) {
        return Err(format!("发现重复键 {key:?}"));
    }
    entries.push((key, value));
    Ok(())
}

fn split_anchor(raw: &str) -> (Option<String>, &str) {
    let raw = raw.trim_start();
    let Some(rest) = raw.strip_prefix('&') else {
        return (None, raw);
    };
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest[..end].trim();
    if name.is_empty() {
        return (None, raw);
    }
    (Some(name.to_owned()), rest[end..].trim_start())
}

fn is_sequence_entry(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

fn is_block_scalar(value: &str) -> bool {
    matches!(value.trim_start().chars().next(), Some('|' | '>'))
}

fn fold_lines(lines: &[String]) -> String {
    let mut result = String::new();
    let mut previous_was_blank = true;
    for line in lines {
        if line.is_empty() {
            if !result.ends_with('\n') {
                result.push('\n');
            }
            previous_was_blank = true;
            continue;
        }
        if !previous_was_blank && !result.ends_with('\n') {
            result.push(' ');
        }
        result.push_str(line);
        previous_was_blank = false;
    }
    result
}
