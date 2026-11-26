use std::collections::BTreeMap;

use reqwest::{
    Response,
    header::{CONTENT_TYPE, HeaderMap, HeaderValue, SET_COOKIE},
};
use serde_json::Value;
use uuid::Uuid;

use super::SessionError;

pub(super) const JSON_LIMIT: usize = 16 * 1024 * 1024;

pub(super) fn request_id(headers: &HeaderMap) -> Option<String> {
    let ids = headers.get_all("x-request-id");
    let mut values = ids.iter();
    let candidate = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let id = Uuid::parse_str(candidate).ok()?;
    (id.get_version_num() == 7
        && id.to_string() == candidate
        && matches!(
            candidate.as_bytes().get(19),
            Some(b'8' | b'9' | b'a' | b'b')
        ))
    .then(|| candidate.to_owned())
}

pub(super) fn cookies(
    headers: &HeaderMap,
    jar: &mut BTreeMap<String, String>,
    operation: &str,
) -> Result<(), SessionError> {
    for header in headers.get_all(SET_COOKIE) {
        let raw = header.to_str().map_err(|_| invalid(operation))?;
        let pair = raw.split(';').next().ok_or_else(|| invalid(operation))?;
        let (name, value) = pair.split_once('=').ok_or_else(|| invalid(operation))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || !value
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b';')
        {
            return Err(invalid(operation));
        }
        let deleted = value.is_empty()
            || raw.split(';').skip(1).any(|attribute| {
                attribute
                    .trim()
                    .split_once('=')
                    .is_some_and(|(key, value)| {
                        key.eq_ignore_ascii_case("max-age") && value.trim() == "0"
                    })
            });
        if deleted {
            jar.remove(name);
        } else {
            jar.insert(name.into(), value.into());
        }
    }
    Ok(())
}

pub(super) async fn json(mut reply: Response, operation: &str) -> Result<Value, SessionError> {
    let status = reply.status();
    if !status.is_success() {
        return Err(SessionError::new(
            "http_status",
            Some(operation),
            Some(status.as_u16()),
        ));
    }
    if status.as_u16() == 204 {
        return Ok(Value::Null);
    }
    let media = reply
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if !media.is_some_and(|value| value.to_ascii_lowercase().contains("json"))
        || reply
            .content_length()
            .is_some_and(|length| length > JSON_LIMIT as u64)
    {
        return Err(invalid(operation));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|error| {
        let category = if error.is_timeout() {
            "request_timeout"
        } else {
            "invalid_response"
        };
        SessionError::new(category, Some(operation), None)
    })? {
        if chunk.len() > JSON_LIMIT.saturating_sub(bytes.len()) {
            return Err(invalid(operation));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid(operation))?;
    if value
        .get("code")
        .is_some_and(|code| code.as_f64() != Some(200.0))
    {
        return Err(SessionError::new(
            "business_response",
            Some(operation),
            None,
        ));
    }
    Ok(value)
}

pub(super) fn token(value: &Value, operation: &str) -> Result<String, SessionError> {
    let token = value
        .pointer("/data/access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic()))
        .ok_or_else(|| invalid(operation))?;
    HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| invalid(operation))?;
    Ok(token.into())
}

fn invalid(operation: &str) -> SessionError {
    SessionError::new("invalid_response", Some(operation), None)
}
