use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat};
use regex::Regex;
use serde_json::{Map, Value};

use super::json;
use crate::Result;

const FIELDS: &[&str] = &[
    "ContentType",
    "CacheControl",
    "ContentDisposition",
    "ContentEncoding",
    "ContentLanguage",
    "Expires",
    "WebsiteRedirectLocation",
    "Metadata",
];

pub(crate) fn copy(value: &Value) -> Result<Value> {
    let object = json::exact(value, FIELDS, "复制对象 metadata")?;
    let header = Regex::new(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$")?;
    let parameter = Regex::new(
        r#"; *([!#$%&'*+.^_`|~0-9A-Za-z-]+)=(?:[!#$%&'*+.^_`|~0-9A-Za-z-]+|"(?:[ !#-\[\]-~]|\\[ -~])*")"#,
    )?;
    let media = Regex::new(
        r#"^([!#$%&'*+.^_`|~0-9A-Za-z-]+)/([!#$%&'*+.^_`|~0-9A-Za-z-]+)((?:; *[!#$%&'*+.^_`|~0-9A-Za-z-]+=(?:[!#$%&'*+.^_`|~0-9A-Za-z-]+|"(?:[ !#-\[\]-~]|\\[ -~])*"))*)$"#,
    )?;
    let mut result = Map::new();
    for field in FIELDS {
        let Some(value) = object.get(*field) else {
            return Err("复制对象字段缺失或包含未知字段".into());
        };
        if *field == "Metadata" {
            continue;
        }
        if value.is_null() && *field != "ContentType" {
            result.insert((*field).into(), Value::Null);
            continue;
        }
        let text = value.as_str().ok_or("复制对象响应头必须为字符串或 NULL")?;
        if text.is_empty()
            || text.trim() != text
            || text.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))
        {
            return Err("复制对象响应头必须是明确的可打印 ASCII 值，禁止控制字符".into());
        }
        result.insert((*field).into(), Value::String(text.to_owned()));
    }
    let content_type = result
        .get("ContentType")
        .and_then(Value::as_str)
        .ok_or("复制对象 ContentType 必须是完整媒体类型")?;
    let captured = media
        .captures(content_type)
        .ok_or("复制对象 ContentType 必须是完整媒体类型")?;
    if captured[1].contains('*') || captured[2].contains('*') {
        return Err("复制对象 ContentType 必须是完整媒体类型".into());
    }
    let names = parameter
        .captures_iter(content_type)
        .map(|capture| capture[1].to_ascii_lowercase())
        .collect::<Vec<_>>();
    if names
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != names.len()
    {
        return Err("复制对象 ContentType 参数重复，不能猜测其含义".into());
    }
    if let Some(expires) = result.get("Expires").and_then(Value::as_str) {
        let pattern = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})$")?;
        if !pattern.is_match(expires) {
            return Err("复制对象 Expires 必须是带时区且精确到秒的 RFC3339".into());
        }
        let instant = DateTime::parse_from_rfc3339(expires)
            .map_err(|_| "复制对象 Expires 必须是带时区且精确到秒的 RFC3339")?;
        result.insert(
            "Expires".into(),
            Value::String(instant.to_rfc3339_opts(SecondsFormat::Secs, false)),
        );
    }
    let custom = object
        .get("Metadata")
        .ok_or("复制对象 Metadata 必须显式为空字典或完整键值字典")?;
    let custom = json::object(custom, "复制对象 Metadata")?;
    let mut metadata = BTreeMap::new();
    let mut total = 0_usize;
    for (key, value) in custom {
        let value = value
            .as_str()
            .ok_or("复制对象 Metadata 值必须为可打印 ASCII 字符串")?;
        if !header.is_match(key)
            || key != &key.to_ascii_lowercase()
            || value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))
        {
            return Err("复制对象 Metadata 必须使用唯一小写头名称和可打印 ASCII 值".into());
        }
        total = total
            .checked_add(key.len() + value.len())
            .ok_or("复制对象 Metadata 大小溢出")?;
        metadata.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    if total > 2048 {
        return Err("复制对象自定义 Metadata 超过 2 KiB".into());
    }
    result.insert(
        "Metadata".into(),
        Value::Object(metadata.into_iter().collect()),
    );
    Ok(Value::Object(result))
}
