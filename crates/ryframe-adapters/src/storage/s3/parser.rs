use quick_xml::{Reader, escape::unescape, events::Event};
use serde_json::Value;

use super::super::{ObjectListPage, StorageError, StorageResult, key_segments};

pub fn parse_list_objects_response(
    body: &[u8],
    expected_prefix: &str,
    limit: usize,
) -> StorageResult<ObjectListPage> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut keys = Vec::new();
    let mut next_cursor = None;
    let mut is_truncated = None;
    loop {
        match reader.read_event().map_err(|error| {
            StorageError::InvalidResponse(format!("invalid S3 object list XML: {error}"))
        })? {
            Event::Start(element) if element.local_name().as_ref() == b"Key" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!("invalid S3 object key element: {error}"))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 object key encoding: {error}"
                    ))
                })?;
                let key = unescape(&decoded)
                    .map_err(|error| {
                        StorageError::InvalidResponse(format!(
                            "invalid S3 object key escaping: {error}"
                        ))
                    })?
                    .into_owned();
                key_segments(&key)?;
                if !key.starts_with(expected_prefix) {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list returned a key outside the requested prefix".to_owned(),
                    ));
                }
                if keys.last().is_some_and(|previous| previous >= &key) {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list keys are not strictly ordered".to_owned(),
                    ));
                }
                keys.push(key);
                if keys.len() > limit {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list returned more keys than requested".to_owned(),
                    ));
                }
            }
            Event::Start(element) if element.local_name().as_ref() == b"NextContinuationToken" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 continuation token element: {error}"
                    ))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 continuation token encoding: {error}"
                    ))
                })?;
                let token = unescape(&decoded)
                    .map_err(|error| {
                        StorageError::InvalidResponse(format!(
                            "invalid S3 continuation token escaping: {error}"
                        ))
                    })?
                    .into_owned();
                if token.is_empty() || token.len() > 4_096 || token.chars().any(char::is_control) {
                    return Err(StorageError::InvalidResponse(
                        "S3 continuation token is invalid".to_owned(),
                    ));
                }
                next_cursor = Some(token);
            }
            Event::Start(element) if element.local_name().as_ref() == b"IsTruncated" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!("invalid S3 truncation element: {error}"))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 truncation flag encoding: {error}"
                    ))
                })?;
                is_truncated = Some(match decoded.as_ref() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(StorageError::InvalidResponse(
                            "S3 truncation flag must be true or false".to_owned(),
                        ));
                    }
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    match is_truncated {
        Some(true) if next_cursor.is_none() => Err(StorageError::InvalidResponse(
            "truncated S3 object list has no continuation token".to_owned(),
        )),
        Some(false) => Ok(ObjectListPage {
            keys,
            next_cursor: None,
        }),
        Some(true) => Ok(ObjectListPage { keys, next_cursor }),
        None => Err(StorageError::InvalidResponse(
            "S3 object list has no truncation flag".to_owned(),
        )),
    }
}

/// 保守地拒绝存储桶策略中的匿名或公开授权。
///
/// S3 策略可通过不止 `Principal: "*" + Action: "s3:GetObject"` 一种形式表达公开访问。
/// 尤其是，带有 `NotPrincipal` 的 `Allow` 语句会向除列出主体外的所有人授权，而 `NotAction`
/// 也可能间接授予读取权限。RyFrame 的私有文件约定不需要这两种形式，因此采取失败即拒绝策略，
/// 而不在此复刻完整的 IAM 策略求值器。
pub(super) fn policy_allows_public_access(policy: &Value) -> bool {
    let statements = match policy.get("Statement") {
        Some(Value::Array(statements)) => statements.iter().collect::<Vec<_>>(),
        Some(statement @ Value::Object(_)) => vec![statement],
        _ => return false,
    };
    statements.into_iter().any(|statement| {
        statement.get("Effect").and_then(Value::as_str) == Some("Allow")
            && (statement.get("NotPrincipal").is_some()
                || value_contains_wildcard(statement.get("Principal")))
    })
}

fn value_contains_wildcard(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => value == "*",
        Some(Value::Array(values)) => values
            .iter()
            .any(|value| value_contains_wildcard(Some(value))),
        Some(Value::Object(values)) => values
            .values()
            .any(|value| value_contains_wildcard(Some(value))),
        _ => false,
    }
}
