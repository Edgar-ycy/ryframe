use super::{
    Input, artifacts,
    failure::{Failure, NativeResult},
};
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn fingerprint(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(digest)
}
pub(crate) fn hash(value: &Value) -> NativeResult<String> {
    // serde_json 的默认 Map 使用有序键，与原递归 canonical 一致。
    Ok(artifacts::sha(
        &serde_json::to_vec(value).map_err(|_| Failure::new("configuration_hash"))?,
    ))
}
pub(crate) fn read(bytes: &[u8], input: &Input) -> NativeResult<Value> {
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::new("runtime_configuration"))?;
    let config = crate::recovery::clone::native::json::parse_fragment(text, "运行时配置")
        .map_err(|_| Failure::new("runtime_configuration"))?;
    super::super::runtime_configuration::validate(&config, &input.suite)
        .map_err(|_| Failure::new("runtime_configuration"))?;
    if hash(&config["contract"])? != input.input_sha256
        || config["bindings"]["source_fingerprints"] != json!(input.source_fingerprints)
        || config["bindings"]["source_fingerprints"]["runner_frontend"]
            != input.runner_frontend_fingerprint
    {
        return Err(Failure::new("runtime_input_binding"));
    }
    selection_config(&config)?;
    if config["bindings"].get("metrics_token_env").is_some() {
        return Err(Failure::new("file_configuration_required"));
    }
    Ok(config)
}

/// 校验私有文件中的身份绑定，纯槽位模型直接使用文件字段。
pub(crate) fn selection_config(config: &Value) -> NativeResult<Value> {
    let mut projected = config.clone();
    let pools = projected["bindings"]["identity_pools"]
        .as_object_mut()
        .ok_or_else(|| Failure::new("identity_selection"))?;
    for pool in pools.values_mut() {
        for identity in pool
            .as_array_mut()
            .ok_or_else(|| Failure::new("identity_selection"))?
        {
            let fields = identity
                .as_object_mut()
                .ok_or_else(|| Failure::new("identity_selection"))?;
            if fields.len() != 3
                || !fields.contains_key("tenant_id")
                || !fields.contains_key("username")
                || fields
                    .get("password")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                return Err(Failure::new("file_identity_binding"));
            }
        }
    }
    Ok(projected)
}

pub(crate) fn identities(
    config: &Value,
    suite: &str,
    workflow: &str,
    count: u64,
) -> NativeResult<Vec<Value>> {
    let projected = selection_config(config)?;
    let selected = if suite == "homepage" {
        super::super::runtime_selection::homepage(&projected, count)
    } else {
        super::super::runtime_selection::workload(&projected, suite, workflow, count)
    }
    .map_err(|_| Failure::new("identity_selection"))?;
    selected["identities"]
        .as_array()
        .cloned()
        .ok_or_else(|| Failure::new("identity_selection"))
}

pub(crate) fn catalog(root: &Path) -> NativeResult<BTreeMap<String, Value>> {
    let bytes = artifacts::read(&root.join("openapi/openapi.json"), 16 * 1024 * 1024)?;
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| Failure::new("operation_catalog"))?;
    super::super::runtime_configuration::operation_catalog(&document)
        .map_err(|_| Failure::new("operation_catalog"))
}

pub(crate) fn pointer<'a>(value: &'a Value, path: &str) -> NativeResult<&'a Value> {
    if !path.starts_with('/') {
        return Err(Failure::new("response_pointer"));
    }
    value
        .pointer(path)
        .ok_or_else(|| Failure::new("response_pointer"))
}

pub(crate) fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(rows) => rows
            .iter()
            .map(|row| {
                if row.is_null() {
                    String::new()
                } else {
                    scalar(row)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        value => value.to_string(),
    }
}

pub(crate) fn expand(value: &Value, variables: &Value) -> NativeResult<Value> {
    match value {
        Value::String(text) => {
            let pattern =
                Regex::new(r"\$\{([a-zA-Z0-9_]+)\}").map_err(|_| Failure::new("variable_model"))?;
            if let Some(captures) = pattern
                .captures(text)
                .filter(|capture| capture.get(0).is_some_and(|all| all.as_str() == text))
            {
                return variables
                    .get(&captures[1])
                    .cloned()
                    .ok_or_else(|| Failure::new("missing_variable"));
            }
            let mut output = String::new();
            let mut end = 0;
            for captures in pattern.captures_iter(text) {
                let found = captures
                    .get(0)
                    .ok_or_else(|| Failure::new("variable_model"))?;
                output.push_str(&text[end..found.start()]);
                output.push_str(&scalar(
                    variables
                        .get(&captures[1])
                        .ok_or_else(|| Failure::new("missing_variable"))?,
                ));
                end = found.end();
            }
            output.push_str(&text[end..]);
            Ok(json!(output))
        }
        Value::Array(rows) => rows
            .iter()
            .map(|row| expand(row, variables))
            .collect::<NativeResult<Vec<_>>>()
            .map(Value::Array),
        Value::Object(rows) => rows
            .iter()
            .map(|(key, value)| Ok((key.clone(), expand(value, variables)?)))
            .collect::<NativeResult<serde_json::Map<_, _>>>()
            .map(Value::Object),
        value => Ok(value.clone()),
    }
}
