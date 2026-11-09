use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Document {
    pub(crate) path: PathBuf,
    pub(crate) raw: Vec<u8>,
    pub(crate) value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

impl Document {
    pub(crate) fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.raw))
    }

    pub(crate) fn assert_unchanged(&self) -> Result<()> {
        let current = read_native(&self.path, "恢复核验输入")?;
        if current.raw != self.raw || current.path != self.path {
            return Err(format!("核验期间 JSON 输入被替换或修改：{}", self.path.display()).into());
        }
        Ok(())
    }
}

pub(crate) fn read(path: &Path, backend: &Path, label: &str) -> Result<Document> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("{label}无效：{error}"))?;
    read_native(path, label)
}

fn read_native(path: &Path, label: &str) -> Result<Document> {
    crate::recovery::runtime::dataset_authority::reads::file(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{label}必须是普通文件：{error}"))?;
    if !metadata.is_file() || linked(&metadata) {
        return Err(format!("{label}必须是普通文件：{}", path.display()).into());
    }
    if metadata.len() == 0 {
        return Err(format!("{label}不能为空：{}", path.display()).into());
    }
    if metadata.len() > MAX_JSON_BYTES {
        return Err(format!("{label}超过 16 MiB：{}", path.display()).into());
    }
    let raw = fs::read(path).map_err(|error| format!("无法读取{label}：{error}"))?;
    let repeated = fs::read(path).map_err(|error| format!("无法复核{label}：{error}"))?;
    if raw != repeated || metadata.len() != raw.len() as u64 || raw.len() as u64 > MAX_JSON_BYTES {
        return Err(format!("{label}在读取期间被替换或修改：{}", path.display()).into());
    }
    let text = std::str::from_utf8(&raw).map_err(|_| format!("{label}不是 UTF-8"))?;
    reject_duplicate_keys(text, label)?;
    let value: Value = serde_json::from_str(text).map_err(|_| format!("{label}不是严格 JSON"))?;
    if !value.is_object() {
        return Err(format!("{label}必须是 JSON 对象").into());
    }
    Ok(Document {
        path: path.to_path_buf(),
        raw,
        value,
    })
}

pub(crate) fn binding(document: &Document) -> Value {
    serde_json::json!({
        "path": document.path.to_string_lossy(),
        "bytes": document.raw.len() as u64,
        "sha256": document.sha256(),
    })
}

pub(crate) fn bound_file(
    backend: &Path,
    value: &Value,
    label: &str,
) -> Result<(Binding, Document)> {
    let binding = binding_shape(value, label)?;
    let document = read(&binding.path, backend, label)?;
    if document.raw.len() as u64 != binding.bytes || document.sha256() != binding.sha256 {
        return Err(format!("{label}与登记摘要不一致").into());
    }
    Ok((binding, document))
}

pub(crate) fn binding_shape(value: &Value, label: &str) -> Result<Binding> {
    let object = exact(value, &["path", "bytes", "sha256"], label)?;
    let path = PathBuf::from(text(object.get("path").unwrap(), &format!("{label}路径"))?);
    let bytes = integer(object.get("bytes").unwrap(), &format!("{label}字节数"))?;
    let sha256 = digest(object.get("sha256").unwrap(), &format!("{label}摘要"))?;
    if !path.is_absolute() || bytes == 0 {
        return Err(format!("{label}路径或大小无效").into());
    }
    Ok(Binding {
        path,
        bytes,
        sha256,
    })
}

pub(crate) fn hash(value: &Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

pub(crate) fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut content = serde_json::to_string_pretty(value)?.into_bytes();
    content.push(b'\n');
    if content.len() as u64 > MAX_JSON_BYTES {
        return Err("正式恢复输入不能超过 16 MiB".into());
    }
    Ok(content)
}

pub(crate) fn write_pending_and_link(target: &Path, value: &Value, pending: &Path) -> Result<bool> {
    let content = canonical_bytes(value)?;
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pending)
        .map_err(|error| {
            format!(
                "无法创建正式恢复输入临时文件 {}：{error}",
                pending.display()
            )
        })?;
    stream.write_all(&content)?;
    stream.sync_all()?;
    drop(stream);
    verify_bytes(pending, value, &content)?;
    match fs::hard_link(pending, target) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("正式恢复输入已被并发创建，禁止覆盖".into())
        }
        Err(error) => Err(format!("无法以 create-only 语义发布正式恢复输入：{error}").into()),
    }
}

pub(crate) fn verify_published(target: &Path, value: &Value, content: &[u8]) -> Result<()> {
    verify_bytes(target, value, content)?;
    let canonical =
        fs::canonicalize(target).map_err(|error| format!("无法解析已发布正式恢复输入：{error}"))?;
    let expected = target
        .parent()
        .ok_or("正式恢复输入缺少父目录")?
        .canonicalize()?
        .join(target.file_name().ok_or("正式恢复输入缺少文件名")?);
    if canonical != expected {
        return Err("正式恢复输入写后规范路径发生变化".into());
    }
    Ok(())
}

fn verify_bytes(path: &Path, value: &Value, content: &[u8]) -> Result<()> {
    let observed = fs::read(path)?;
    if observed != content {
        return Err("正式恢复输入写后字节与规范结果不同".into());
    }
    let parsed: Value =
        serde_json::from_slice(&observed).map_err(|_| "正式恢复输入写后不是有效 JSON")?;
    if &parsed != value {
        return Err("正式恢复输入写后内容与规范结果不同".into());
    }
    Ok(())
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

pub(crate) fn integer(value: &Value, label: &str) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| format!("{label}必须为非负整数").into())
}

pub(crate) fn digest(value: &Value, label: &str) -> Result<String> {
    let value = text(value, label)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label}格式无效").into());
    }
    Ok(value.to_owned())
}

fn reject_duplicate_keys(source: &str, label: &str) -> Result<()> {
    let mut scanner = Scanner {
        bytes: source.as_bytes(),
        position: 0,
        label,
    };
    scanner.value(0)
}

struct Scanner<'a> {
    bytes: &'a [u8],
    position: usize,
    label: &'a str,
}

impl Scanner<'_> {
    fn value(&mut self, depth: usize) -> Result<()> {
        if depth > 256 {
            return Err(format!("{}嵌套超过限制", self.label).into());
        }
        self.whitespace();
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string(),
            Some(_) => self.primitive(),
            None => Err(format!("{} JSON 值缺失", self.label).into()),
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
            let key: String = serde_json::from_slice(&self.bytes[start..self.position])
                .map_err(|_| format!("{}对象字段不是有效 JSON 字符串", self.label))?;
            if !keys.insert(key.clone()) {
                return Err(format!("{}包含重复 JSON 字段：{key}", self.label).into());
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
        let mut escaped = false;
        while let Some(byte) = self.peek() {
            self.position += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(());
            }
        }
        Err(format!("{}字符串未结束", self.label).into())
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
            Err(format!("{} JSON 值无效", self.label).into())
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
            Err(format!("{} JSON 结构无效", self.label).into())
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
pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
