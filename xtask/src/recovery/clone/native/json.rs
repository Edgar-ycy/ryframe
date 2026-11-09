use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use serde_json::{Map, Value};

use crate::Result;

pub(crate) const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn read(path: &Path) -> Result<Value> {
    crate::recovery::runtime::dataset_authority::reads::file(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || linked_metadata(&metadata) {
        return Err("复制清单必须是普通文件，不能经过链接或 junction".into());
    }
    if metadata.len() > MAX_JSON_BYTES {
        return Err("复制清单超过 16 MiB".into());
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err("复制清单超过 16 MiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "复制清单必须是 UTF-8 JSON")?;
    let value = parse_fragment_with_label(text, "复制清单")?;
    if !value.is_object() {
        return Err("复制清单必须为对象".into());
    }
    Ok(value)
}

pub(crate) fn parse_fragment(text: &str, label: &str) -> Result<Value> {
    parse_fragment_with_label(text, label)
}

fn parse_fragment_with_label(text: &str, label: &str) -> Result<Value> {
    let value =
        serde_json::from_str(text).map_err(|error| format!("{label}不是有效 JSON：{error}"))?;
    reject_duplicate_keys(text)?;
    Ok(value)
}

pub(crate) fn write_new(path: &Path, value: &Value) -> Result<()> {
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法创建新计划 {}：{error}", path.display()))?;
    let mut encoded = serde_json::to_string_pretty(value)?;
    encoded.push('\n');
    stream.write_all(encoded.as_bytes())?;
    stream.sync_all()?;
    Ok(())
}

pub(crate) fn write_replace(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or("复制状态文件缺少父目录")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("复制状态文件名必须为 UTF-8")?;
    let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut stream = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                format!("无法创建复制状态临时文件 {}：{error}", temporary.display())
            })?;
        let mut encoded = serde_json::to_string_pretty(value)?;
        encoded.push('\n');
        stream.write_all(encoded.as_bytes())?;
        stream.sync_all()?;
        drop(stream);
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn replace(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)
        .map_err(|error| format!("无法发布复制状态 {}：{error}", target.display()).into())
}

pub(crate) fn exact<'a>(
    value: &'a Value,
    fields: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label}必须为对象"))?;
    let expected = fields.iter().copied().collect::<HashSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<HashSet<_>>();
    if expected != actual {
        return Err(format!("{label}字段缺失或包含未知字段").into());
    }
    Ok(object)
}

pub(crate) fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{label}必须为对象").into())
}

pub(crate) fn array<'a>(value: &'a Value, label: &str) -> Result<&'a [Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{label}必须为数组").into())
}

pub(crate) fn text<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| format!("{label}必须为字符串").into())
}

pub(crate) fn boolean(value: &Value, label: &str) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| format!("{label}必须为布尔值").into())
}

fn reject_duplicate_keys(source: &str) -> Result<()> {
    let mut scanner = Scanner {
        source,
        bytes: source.as_bytes(),
        position: 0,
    };
    scanner.value(0)
}

struct Scanner<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
}

impl Scanner<'_> {
    fn value(&mut self, depth: usize) -> Result<()> {
        if depth > 256 {
            return Err("复制清单嵌套超过限制".into());
        }
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(|_| ()),
            Some(_) => self.primitive(),
            None => Err("复制清单 JSON 值缺失".into()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<()> {
        self.position += 1;
        self.whitespace();
        if self.take(b'}') {
            return Ok(());
        }
        let mut keys = HashSet::new();
        loop {
            self.whitespace();
            let start = self.position;
            self.string()?;
            let key: String = serde_json::from_str(&self.source[start..self.position])
                .map_err(|_| "复制清单对象字段不是有效 JSON 字符串")?;
            if !keys.insert(key) {
                return Err("复制清单包含重复 JSON 字段".into());
            }
            self.whitespace();
            self.expect(b':')?;
            self.value(depth)?;
            self.whitespace();
            if self.take(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self, depth: usize) -> Result<()> {
        self.position += 1;
        self.whitespace();
        if self.take(b']') {
            return Ok(());
        }
        loop {
            self.value(depth)?;
            self.whitespace();
            if self.take(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<()> {
        self.expect(b'"')?;
        while let Some(byte) = self.peek() {
            self.position += 1;
            match byte {
                b'"' => return Ok(()),
                b'\\' => {
                    self.position = self
                        .position
                        .checked_add(1)
                        .filter(|position| *position <= self.bytes.len())
                        .ok_or("复制清单字符串转义未结束")?;
                }
                _ => {}
            }
        }
        Err("复制清单字符串未结束".into())
    }

    fn primitive(&mut self) -> Result<()> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() || matches!(byte, b',' | b']' | b'}') {
                break;
            }
            self.position += 1;
        }
        if self.position == start {
            Err("复制清单 JSON 值无效".into())
        } else {
            Ok(())
        }
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.take(expected) {
            Ok(())
        } else {
            Err("复制清单 JSON 结构无效".into())
        }
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
}

#[cfg(windows)]
fn linked_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn linked_metadata(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
