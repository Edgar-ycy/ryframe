use serde_json::Value;
use url::Url;

pub(super) fn valid(route: &str) -> bool {
    route.starts_with('/')
        && !route.contains("//")
        && !route
            .chars()
            .any(|value| value.is_control() || value.is_whitespace() || "\\?#%".contains(value))
        && !route.split('/').any(|part| matches!(part, "." | ".."))
        && expand(route, Some(&Value::Object(placeholders(route)))).is_ok()
}

fn placeholders(route: &str) -> serde_json::Map<String, Value> {
    route
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}'))
        .map(|(key, _)| (key.into(), Value::String("parameter".into())))
        .collect()
}

pub(super) fn expand(template: &str, parameters: Option<&Value>) -> Result<String, ()> {
    let empty = serde_json::Map::new();
    let parameters = match parameters {
        Some(value) => value.as_object().ok_or(())?,
        None => &empty,
    };
    let mut remaining = template;
    let mut result = String::new();
    while let Some((prefix, suffix)) = remaining.split_once('{') {
        if prefix.contains('}') {
            return Err(());
        }
        let (key, rest) = suffix.split_once('}').ok_or(())?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(());
        }
        let value = scalar(parameters.get(key).ok_or(())?).ok_or(())?;
        if matches!(value.as_str(), "." | "..") {
            return Err(());
        }
        result.push_str(prefix);
        result.push_str(&encode_component(&value));
        remaining = rest;
    }
    if remaining.contains('}') {
        return Err(());
    }
    result.push_str(remaining);
    Ok(result)
}

pub(super) fn query(url: &mut Url, query: Option<&Value>) -> Result<(), ()> {
    let Some(query) = query else { return Ok(()) };
    let query = query.as_object().ok_or(())?;
    for (key, value) in query {
        if value.is_null() {
            continue;
        }
        url.query_pairs_mut()
            .append_pair(key, &scalar(value).ok_or(())?);
    }
    Ok(())
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("字符串写入不会失败");
        }
    }
    encoded
}
