use std::collections::BTreeMap;

use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Operation;
use crate::Result;

pub(super) fn metadata(headers: &HeaderMap) -> Result<Value> {
    check_headers(headers)?;
    let mut result = serde_json::Map::new();
    for (header, field) in [
        ("content-type", "ContentType"),
        ("etag", "ETag"),
        ("accept-ranges", "AcceptRanges"),
        ("cache-control", "CacheControl"),
        ("content-disposition", "ContentDisposition"),
        ("content-encoding", "ContentEncoding"),
        ("content-language", "ContentLanguage"),
        ("x-amz-version-id", "VersionId"),
        ("x-amz-storage-class", "StorageClass"),
        ("x-amz-website-redirect-location", "WebsiteRedirectLocation"),
        ("x-amz-server-side-encryption", "ServerSideEncryption"),
        ("x-amz-server-side-encryption-aws-kms-key-id", "SSEKMSKeyId"),
        ("x-amz-checksum-sha256", "ChecksumSHA256"),
        ("x-amz-checksum-sha1", "ChecksumSHA1"),
        ("x-amz-checksum-crc32", "ChecksumCRC32"),
        ("x-amz-checksum-crc32c", "ChecksumCRC32C"),
        ("x-amz-checksum-crc64nvme", "ChecksumCRC64NVME"),
        ("x-amz-checksum-type", "ChecksumType"),
        ("x-amz-replication-status", "ReplicationStatus"),
        ("x-amz-expiration", "Expiration"),
        ("x-amz-restore", "Restore"),
        ("x-amz-object-lock-mode", "ObjectLockMode"),
        (
            "x-amz-object-lock-retain-until-date",
            "ObjectLockRetainUntilDate",
        ),
        ("x-amz-object-lock-legal-hold", "ObjectLockLegalHoldStatus"),
    ] {
        if let Some(value) = text(headers, header)? {
            result.insert(field.into(), json!(value));
        }
    }
    for (header, field) in [
        ("content-length", "ContentLength"),
        ("x-amz-missing-meta", "MissingMeta"),
        ("x-amz-tagging-count", "TagCount"),
        ("x-amz-mp-parts-count", "PartsCount"),
    ] {
        if let Some(value) = text(headers, header)? {
            result.insert(
                field.into(),
                json!(value.parse::<u64>().map_err(|_| "S3 数值响应头无效")?),
            );
        }
    }
    for (header, field) in [("last-modified", "LastModified"), ("expires", "Expires")] {
        if let Some(value) = text(headers, header)? {
            let date =
                chrono::DateTime::parse_from_rfc2822(value).map_err(|_| "S3 时间响应头无效")?;
            result.insert(field.into(), json!(date.to_utc().to_rfc3339()));
        }
    }
    let mut metadata = BTreeMap::new();
    for name in headers.keys() {
        if let Some(key) = name.as_str().strip_prefix("x-amz-meta-") {
            metadata.insert(
                key.to_owned(),
                text(headers, name.as_str())?.ok_or("S3 元数据头缺失")?,
            );
        }
    }
    result.insert("Metadata".into(), json!(metadata));
    for required in ["ContentLength", "ETag", "LastModified", "ContentType"] {
        if !result.contains_key(required) {
            return Err(format!("S3 对象响应缺少 {required}").into());
        }
    }
    Ok(Value::Object(result))
}

fn check_headers(headers: &HeaderMap) -> Result<()> {
    const OBSERVED: &[&str] = &[
        "x-amz-version-id",
        "x-amz-storage-class",
        "x-amz-website-redirect-location",
        "x-amz-server-side-encryption",
        "x-amz-server-side-encryption-aws-kms-key-id",
        "x-amz-checksum-sha256",
        "x-amz-checksum-sha1",
        "x-amz-checksum-crc32",
        "x-amz-checksum-crc32c",
        "x-amz-checksum-crc64nvme",
        "x-amz-checksum-type",
        "x-amz-replication-status",
        "x-amz-expiration",
        "x-amz-restore",
        "x-amz-object-lock-mode",
        "x-amz-object-lock-retain-until-date",
        "x-amz-object-lock-legal-hold",
        "x-amz-missing-meta",
        "x-amz-tagging-count",
        "x-amz-mp-parts-count",
        "x-amz-id-2",
        "x-amz-request-id",
        "x-amz-bucket-region",
    ];
    if headers.contains_key("content-range")
        || headers.keys().any(|name| {
            let name = name.as_str();
            name.starts_with("x-amz-")
                && !name.starts_with("x-amz-meta-")
                && !OBSERVED.contains(&name)
        })
    {
        return Err("S3 包含未知对象策略或部分内容响应头，不能忽略后继续采集".into());
    }
    Ok(())
}

fn text<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>> {
    let values = headers.get_all(name).iter().collect::<Vec<_>>();
    if values.len() > 1 {
        return Err("S3 响应包含重复的关键头，无法确定对象身份".into());
    }
    values
        .first()
        .map(|value| value.to_str().map_err(|_| "S3 响应头不是有效文本".into()))
        .transpose()
}

#[derive(Deserialize)]
#[serde(rename = "VersioningConfiguration", rename_all = "PascalCase")]
struct Versioning {
    status: Option<String>,
    mfa_delete: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "ListBucketResult", rename_all = "PascalCase")]
struct Listing {
    is_truncated: bool,
    encoding_type: String,
    next_continuation_token: Option<String>,
    #[serde(default)]
    contents: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Entry {
    key: String,
}

pub(super) fn xml(bytes: &[u8], operation: &Operation) -> Result<Value> {
    let raw = std::str::from_utf8(bytes).map_err(|_| "S3 XML 不是 UTF-8")?;
    if raw.contains("<!DOCTYPE") || raw.contains("<!ENTITY") {
        return Err("S3 XML 禁止实体与文档类型声明".into());
    }
    let root = match operation {
        Operation::Versioning => "VersioningConfiguration",
        Operation::List { .. } => "ListBucketResult",
        _ => return Err("该 S3 操作不接受 XML 响应".into()),
    };
    validate_root(raw, root)?;
    match operation {
        Operation::Versioning => {
            let value: Versioning =
                quick_xml::de::from_str(raw).map_err(|_| "S3 版本状态 XML 无效")?;
            let mut result = json!({});
            if let Some(status) = value.status {
                result["Status"] = json!(status);
            }
            if let Some(status) = value.mfa_delete {
                result["MFADelete"] = json!(status);
            }
            Ok(result)
        }
        Operation::List { .. } => {
            let value: Listing =
                quick_xml::de::from_str(raw).map_err(|_| "S3 分页 XML 缺少完成状态或结构无效")?;
            if value.encoding_type != "url" {
                return Err("S3 列表未确认 URL 编码，不能确定对象 key".into());
            }
            let entries = value
                .contents
                .into_iter()
                .map(|entry| {
                    let raw = entry.key.as_bytes();
                    for (index, byte) in raw.iter().enumerate() {
                        if *byte == b'%'
                            && !(raw.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                                && raw.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
                        {
                            return Err("S3 key 的百分比编码无效".into());
                        }
                    }
                    let key = percent_encoding::percent_decode_str(&entry.key)
                        .decode_utf8()
                        .map_err(|_| "S3 key 不是 UTF-8")?;
                    Ok(json!({"Key": key}))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut result = json!({"IsTruncated": value.is_truncated, "Contents": entries});
            if let Some(token) = value.next_continuation_token {
                result["NextContinuationToken"] = json!(token);
            }
            Ok(result)
        }
        _ => Err("该 S3 操作不接受 XML 响应".into()),
    }
}

fn validate_root(raw: &str, expected: &str) -> Result<()> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(raw);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0_u32;
    let mut seen = false;
    loop {
        match reader.read_event().map_err(|_| "S3 XML 文档结构无效")? {
            Event::Start(tag) => {
                if depth == 0 {
                    if seen || tag.local_name().as_ref() != expected.as_bytes() {
                        return Err("S3 XML 根节点不匹配或重复".into());
                    }
                    seen = true;
                }
                depth = depth.checked_add(1).ok_or("S3 XML 嵌套溢出")?;
                if depth > 64 {
                    return Err("S3 XML 嵌套超过 64 层".into());
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or("S3 XML 结束节点无效")?;
            }
            Event::DocType(_) => return Err("S3 XML 禁止文档类型声明".into()),
            Event::Text(value) if depth == 0 && !value.iter().all(u8::is_ascii_whitespace) => {
                return Err("S3 XML 根节点之外存在正文".into());
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen || depth != 0 {
        return Err("S3 XML 根节点缺失或未闭合".into());
    }
    Ok(())
}
