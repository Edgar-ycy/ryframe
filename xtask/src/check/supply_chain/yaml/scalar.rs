use std::collections::BTreeMap;

use super::{YamlKey, YamlResult, YamlValue, insert_unique};

pub(super) fn parse_inline(raw: &str) -> YamlResult<YamlValue> {
    parse_inline_with_anchors(raw, &BTreeMap::new())
}

pub(super) fn parse_inline_with_anchors(
    raw: &str,
    anchors: &BTreeMap<String, YamlValue>,
) -> YamlResult<YamlValue> {
    let raw = strip_comment(raw).trim();
    if raw.is_empty() {
        return Ok(YamlValue::Null);
    }
    if matches!(raw.chars().next(), Some('!' | '?' | '%')) {
        return Err("不支持的 YAML 标签、显式键或指令".into());
    }
    if raw.starts_with('{') || raw.starts_with('[') {
        let mut parser = FlowParser::new(raw, anchors);
        let value = parser.parse_value()?;
        parser.skip_space();
        if !parser.at_end() {
            return Err("flow 值后存在无法解析的内容".into());
        }
        return Ok(value);
    }
    parse_scalar(raw)
}

pub(super) fn strip_comment(raw: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in raw.char_indices() {
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
            '#' if index == 0
                || raw[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace) =>
            {
                return raw[..index].trim_end();
            }
            _ => {}
        }
    }
    raw
}

fn parse_scalar(raw: &str) -> YamlResult<YamlValue> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(YamlValue::Null);
    }
    if raw.starts_with('\'') {
        return parse_single_quoted(raw).map(YamlValue::String);
    }
    if raw.starts_with('"') {
        return parse_double_quoted(raw).map(YamlValue::String);
    }
    match raw {
        "null" | "Null" | "NULL" | "~" => return Ok(YamlValue::Null),
        _ => {}
    }
    if raw.eq_ignore_ascii_case("true") {
        return Ok(YamlValue::Bool(true));
    }
    if raw.eq_ignore_ascii_case("false") {
        return Ok(YamlValue::Bool(false));
    }
    if let Ok(integer) = raw.parse::<i64>() {
        return Ok(YamlValue::Integer(integer));
    }
    if let Ok(float) = raw.parse::<f64>()
        && (raw.contains('.') || raw.contains('e') || raw.contains('E'))
    {
        return Ok(YamlValue::Float(float));
    }
    Ok(YamlValue::String(raw.to_owned()))
}

fn parse_single_quoted(raw: &str) -> YamlResult<String> {
    let mut result = String::new();
    let mut characters = raw.char_indices();
    if characters.next().map(|(_, value)| value) != Some('\'') {
        return Err("quoted 标量缺少起始单引号".into());
    }
    let mut previous_quote = false;
    for (index, character) in characters {
        if character == '\'' {
            if previous_quote {
                result.push('\'');
                previous_quote = false;
                continue;
            }
            if raw[index + 1..].trim().is_empty() {
                return Ok(result);
            }
            previous_quote = true;
        } else {
            if previous_quote {
                return Err("单引号字符串后存在无法解析的字符".into());
            }
            result.push(character);
        }
    }
    Err("quoted 标量缺少结束单引号".into())
}

fn parse_double_quoted(raw: &str) -> YamlResult<String> {
    let mut result = String::new();
    let mut escaped = false;
    for (index, character) in raw.char_indices().skip(1) {
        if escaped {
            let escaped_character = match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                _ => return Err("不支持的 YAML 双引号转义序列".into()),
            };
            result.push(escaped_character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => {
                if raw[index + 1..].trim().is_empty() {
                    return Ok(result);
                }
                return Err("双引号字符串后存在无法解析的字符".into());
            }
            other => result.push(other),
        }
    }
    Err("quoted 标量缺少结束双引号".into())
}

struct FlowParser<'a> {
    source: &'a str,
    position: usize,
    anchors: &'a BTreeMap<String, YamlValue>,
}

impl<'a> FlowParser<'a> {
    fn new(source: &'a str, anchors: &'a BTreeMap<String, YamlValue>) -> Self {
        Self {
            source,
            position: 0,
            anchors,
        }
    }

    fn parse_value(&mut self) -> YamlResult<YamlValue> {
        self.skip_space();
        match self.peek() {
            Some('{') => self.parse_mapping(),
            Some('[') => self.parse_sequence(),
            Some('\'') => self.parse_quoted('\'').map(YamlValue::String),
            Some('"') => self.parse_quoted('"').map(YamlValue::String),
            Some('*') => {
                self.bump();
                let alias = self.take_while(|value| {
                    !value.is_whitespace() && !matches!(value, ',' | ']' | '}')
                });
                if alias.is_empty() || alias.chars().any(char::is_whitespace) {
                    return Err("YAML alias 名称无效".into());
                }
                self.anchors
                    .get(alias)
                    .cloned()
                    .ok_or_else(|| format!("未定义的 YAML alias：*{alias}"))
            }
            Some(_) => {
                let raw = self.take_while(|value| !matches!(value, ',' | ']' | '}'));
                parse_scalar(raw.trim())
            }
            None => Ok(YamlValue::Null),
        }
    }

    fn parse_mapping(&mut self) -> YamlResult<YamlValue> {
        self.expect('{')?;
        let mut entries = Vec::new();
        self.skip_space();
        if self.consume('}') {
            return Ok(YamlValue::Mapping(entries));
        }
        loop {
            self.skip_space();
            let key = if matches!(self.peek(), Some('\'' | '"')) {
                let quote = self.peek().unwrap_or('\'');
                YamlKey::String(self.parse_quoted(quote)?)
            } else {
                let raw = self.take_while(|value| value != ':');
                match parse_scalar(raw.trim())? {
                    YamlValue::String(value) => YamlKey::String(value),
                    YamlValue::Bool(value) => YamlKey::Bool(value),
                    YamlValue::Integer(value) => YamlKey::Integer(value),
                    YamlValue::Null => YamlKey::Null,
                    _ => return Err("flow mapping 键必须是标量".into()),
                }
            };
            self.skip_space();
            self.expect(':')?;
            let value = self.parse_value()?;
            insert_unique(&mut entries, key, value)?;
            self.skip_space();
            if self.consume('}') {
                break;
            }
            self.expect(',')?;
        }
        Ok(YamlValue::Mapping(entries))
    }

    fn parse_sequence(&mut self) -> YamlResult<YamlValue> {
        self.expect('[')?;
        let mut values = Vec::new();
        self.skip_space();
        if self.consume(']') {
            return Ok(YamlValue::Sequence(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_space();
            if self.consume(']') {
                break;
            }
            self.expect(',')?;
        }
        Ok(YamlValue::Sequence(values))
    }

    fn parse_quoted(&mut self, quote: char) -> YamlResult<String> {
        self.expect(quote)?;
        let mut result = String::new();
        let mut escaped = false;
        while let Some(character) = self.bump() {
            if quote == '"' && escaped {
                let escaped_character = match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '\'' => '\'',
                    '"' => '"',
                    '\\' => '\\',
                    _ => return Err("不支持的 YAML 双引号转义序列".into()),
                };
                result.push(escaped_character);
                escaped = false;
                continue;
            }
            if quote == '"' && character == '\\' {
                escaped = true;
                continue;
            }
            if character == quote {
                if quote == '\'' && self.peek() == Some('\'') {
                    self.bump();
                    result.push('\'');
                    continue;
                }
                return Ok(result);
            }
            result.push(character);
        }
        Err("flow quoted 标量缺少结束引号".into())
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> &'a str {
        let start = self.position;
        while self.peek().is_some_and(&predicate) {
            self.bump();
        }
        &self.source[start..self.position]
    }

    fn expect(&mut self, expected: char) -> YamlResult<()> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!("flow YAML 缺少 {expected}"))
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn at_end(&self) -> bool {
        self.position >= self.source.len()
    }
}
