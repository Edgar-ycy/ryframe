use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

use crate::Result;

pub(crate) fn validate_absolute(path: &Path, label: &str) -> Result<()> {
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    validate_text_path(path, label)
}

pub(crate) fn validate_text_path(path: &Path, label: &str) -> Result<()> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{label}路径不是有效 UTF-8"))?;
    validate_text(value, label)
}

pub(crate) fn validate_text(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        return Err(format!("{label}不能为空").into());
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{label}不能包含换行符或 NUL").into());
    }
    Ok(())
}

pub(crate) fn canonical_dir(path: &Path, label: &str) -> Result<PathBuf> {
    validate_absolute(path, label)?;
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("无法解析{label} {}：{error}", path.display()))?;
    if !resolved.is_dir() {
        return Err(format!("{label}不是目录：{}", resolved.display()).into());
    }
    Ok(resolved)
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

pub(crate) fn sha256_file(path: &Path, label: &str) -> Result<String> {
    let bytes =
        fs::read(path).map_err(|error| format!("无法读取{label} {}：{error}", path.display()))?;
    Ok(sha256(&bytes))
}

pub(crate) fn sha256_shape(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(crate) fn commit_shape(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(crate) fn read_json(path: &Path, label: &str, limit: usize) -> Result<(Value, Vec<u8>)> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法读取{label} {}：{error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{label}必须是普通文件：{}", path.display()).into());
    }
    let raw =
        fs::read(path).map_err(|error| format!("无法读取{label} {}：{error}", path.display()))?;
    let value = parse_json(&raw, label, limit)?;
    Ok((value, raw))
}

pub(crate) fn parse_json(raw: &[u8], label: &str, limit: usize) -> Result<Value> {
    if raw.is_empty() || raw.len() > limit {
        return Err(format!("{label}大小无效").into());
    }
    let UniqueValue(value) =
        serde_json::from_slice(raw).map_err(|error| format!("{label}不是有效 JSON：{error}"))?;
    if !value.is_object() {
        return Err(format!("{label}必须是 JSON 对象").into());
    }
    Ok(value)
}

pub(crate) fn exact_object<'a>(
    value: &'a Value,
    fields: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label}必须是 JSON 对象"))?;
    let expected = fields.iter().copied().collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!("{label}字段不完整或包含未知内容").into());
    }
    Ok(object)
}

pub(crate) fn string_field<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label}缺少字符串字段 {field}").into())
}

pub(crate) fn positive_integer(value: Option<&Value>, label: &str) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label}必须是正整数").into())
}

pub(crate) fn pretty_json(value: &Value) -> Result<String> {
    let mut output = serde_json::to_string_pretty(value)?;
    output.push('\n');
    Ok(output)
}

pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建输出目录 {}：{error}", parent.display()))?;
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(format!("输出必须是普通文件：{}", path.display()).into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    file.write_all(pretty_json(value)?.as_bytes())
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    Ok(())
}

pub(crate) fn write_new_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建输出目录 {}：{error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("无法新建 {}：{error}", path.display()))?;
    file.write_all(pretty_json(value)?.as_bytes())
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    Ok(())
}

pub(crate) fn read_limited(mut reader: impl Read, limit: usize, label: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(u64::try_from(limit + 1).map_err(|_| format!("{label}上限无效"))?)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取{label}：{error}"))?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(format!("{label}大小无效").into());
    }
    Ok(bytes)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("JSON 数值不是有限值"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        UniqueValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!("包含重复字段：{key}")));
            }
            let UniqueValue(value) = object.next_value()?;
            values.insert(key, value);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}
