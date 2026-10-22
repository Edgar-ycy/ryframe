use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::PathBuf,
};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use super::{PutRequest, object_url};
use crate::Result;

pub(super) struct Prepared {
    pub(super) configuration: super::Configuration,
    pub(super) url: Url,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) source: PathBuf,
    pub(super) expected_bytes: u64,
    pub(super) expected_sha256: String,
}

pub(super) fn prepare(request: PutRequest) -> Result<Prepared> {
    if !request.source.is_absolute()
        || request.source.is_symlink()
        || !request.source.is_file()
        || super::super::digest::linked_path(&request.source)?
            .into_iter()
            .any(|linked| linked)
    {
        return Err("条件创建来源必须是无链接的绝对普通文件；未执行写请求".into());
    }
    if request.expected_sha256.len() != 64
        || !request
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("条件创建来源 SHA256 无效；未执行写请求".into());
    }
    verify_source(
        &request.source,
        request.expected_bytes,
        &request.expected_sha256,
    )?;
    let mut headers = metadata_headers(&request.metadata)?;
    headers.insert("content-length".into(), request.expected_bytes.to_string());
    headers.insert("if-none-match".into(), "*".into());
    let url = object_url(&request.configuration, &request.bucket, Some(&request.key))?;
    Ok(Prepared {
        configuration: request.configuration,
        url,
        headers,
        source: request.source,
        expected_bytes: request.expected_bytes,
        expected_sha256: request.expected_sha256,
    })
}

pub(super) fn verify_prepared_source(value: &Prepared) -> Result<()> {
    verify_source(&value.source, value.expected_bytes, &value.expected_sha256)
}

fn verify_source(path: &PathBuf, expected_bytes: u64, expected_sha256: &str) -> Result<()> {
    let mut file = File::open(path).map_err(|_| "无法读取条件创建来源；未执行写请求")?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "读取条件创建来源失败；未执行写请求")?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or("对象字节数溢出")?;
        digest.update(&buffer[..count]);
    }
    if bytes != expected_bytes || hex::encode(digest.finalize()) != expected_sha256 {
        return Err("条件创建来源与计划字节数或 SHA256 不同；未执行写请求".into());
    }
    Ok(())
}

fn metadata_headers(value: &Value) -> Result<BTreeMap<String, String>> {
    let metadata = value.as_object().ok_or("条件创建 metadata 必须是对象")?;
    let expected = BTreeSet::from([
        "CacheControl",
        "ContentDisposition",
        "ContentEncoding",
        "ContentLanguage",
        "ContentType",
        "Expires",
        "Metadata",
        "WebsiteRedirectLocation",
    ]);
    if metadata.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("条件创建必须完整声明业务响应头与 Metadata；未执行写请求".into());
    }
    let mut headers = BTreeMap::new();
    for (field, header) in [
        ("ContentType", "content-type"),
        ("CacheControl", "cache-control"),
        ("ContentDisposition", "content-disposition"),
        ("ContentEncoding", "content-encoding"),
        ("ContentLanguage", "content-language"),
        ("WebsiteRedirectLocation", "x-amz-website-redirect-location"),
    ] {
        if let Some(text) = optional_ascii(metadata.get(field).unwrap(), field)? {
            if field == "ContentType" {
                content_type(text)?;
            }
            headers.insert(header.into(), text.into());
        }
    }
    if let Some(text) = optional_ascii(metadata.get("Expires").unwrap(), "Expires")? {
        let instant = DateTime::parse_from_rfc3339(text)
            .map_err(|_| "条件创建 Expires 必须是带时区的 RFC3339")?;
        headers.insert(
            "expires".into(),
            instant
                .with_timezone(&Utc)
                .format("%a, %d %b %Y %H:%M:%S GMT")
                .to_string(),
        );
    }
    let custom = metadata["Metadata"]
        .as_object()
        .ok_or("条件创建 Metadata 必须是对象")?;
    let mut total = 0_usize;
    for (key, value) in custom {
        let text = value.as_str().ok_or("条件创建 Metadata 值必须是文本")?;
        if key.is_empty()
            || key
                .bytes()
                .any(|byte| !token(byte) || byte.is_ascii_uppercase())
            || !printable_ascii(text, true)
        {
            return Err("条件创建 Metadata 必须使用小写 token 名称和可打印 ASCII 值".into());
        }
        total = total
            .checked_add(key.len() + text.len())
            .ok_or("条件创建 Metadata 长度溢出")?;
        headers.insert(format!("x-amz-meta-{key}"), text.into());
    }
    if total > 2048 {
        return Err("条件创建 Metadata 超过 2 KiB；未执行写请求".into());
    }
    Ok(headers)
}

fn optional_ascii<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>> {
    if value.is_null() && field != "ContentType" {
        return Ok(None);
    }
    let text = value
        .as_str()
        .filter(|text| !text.is_empty() && text.trim() == *text && printable_ascii(text, false))
        .ok_or_else(|| format!("条件创建 {field} 必须是非空可打印 ASCII 文本"))?;
    Ok(Some(text))
}

fn printable_ascii(value: &str, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty()) && value.bytes().all(|byte| (32..=126).contains(&byte))
}

fn content_type(value: &str) -> Result<()> {
    let mut segments = value.split(';');
    let media = segments.next().unwrap_or_default();
    let (kind, subtype) = media
        .split_once('/')
        .ok_or("条件创建 ContentType 必须是完整媒体类型")?;
    if kind.is_empty()
        || subtype.is_empty()
        || kind.contains('*')
        || subtype.contains('*')
        || !kind.bytes().all(token)
        || !subtype.bytes().all(token)
    {
        return Err("条件创建 ContentType 必须是完整媒体类型".into());
    }
    let mut names = BTreeSet::new();
    for segment in segments {
        let (name, raw) = segment
            .trim()
            .split_once('=')
            .ok_or("条件创建 ContentType 参数无效")?;
        let name = name.to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(token) || !names.insert(name) || !parameter(raw) {
            return Err("条件创建 ContentType 参数重复或无效".into());
        }
    }
    Ok(())
}

fn parameter(value: &str) -> bool {
    if !value.starts_with('"') {
        return !value.is_empty() && value.bytes().all(token);
    }
    if value.len() < 2 || !value.ends_with('"') {
        return false;
    }
    let mut escaped = false;
    for byte in value.as_bytes()[1..value.len() - 1].iter().copied() {
        if escaped {
            if !(32..=126).contains(&byte) {
                return false;
            }
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if !(32..=126).contains(&byte) || byte == b'"' {
            return false;
        }
    }
    !escaped
}

fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}
