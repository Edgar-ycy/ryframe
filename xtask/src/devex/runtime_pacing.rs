use std::collections::BTreeMap;

use serde_json::{Map, Value};

const WINDOW_MARGIN_MS: u64 = 250;
const MAX_INTERVAL_MS: u64 = 86_400_000 + WINDOW_MARGIN_MS;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const CONTRACT: &str = "固定节奏必须声明真实限流摘要、请求/接口间隔与样本准备窗口";
const AUTHORITY_SHAPE: &str = "限流 authority 结构不完整";
const AUTHORITY_LIMIT: &str = "限流 authority 的容量或窗口无效";
const API_AUTHORITY: &str = "接口限流 authority 无效";
const REQUEST_SPACING: &str = "固定请求节奏或样本准备窗口不足以满足实际全局/用户限流";
const UNKNOWN_OPERATION: &str = "节奏声明包含契约中不存在的 operation";
const UNKNOWN_RULE: &str = "限流规则无法用当前契约明确关联操作";
const OPERATION_SPACING: &str = "固定节奏不满足实际接口或登录限流";

struct Pacing<'a> {
    request: u64,
    prepare: u64,
    operations: &'a Map<String, Value>,
}

#[derive(Clone, Copy)]
struct Limit {
    window: u64,
    capacity: u64,
}

impl Limit {
    fn spacing(self) -> u64 {
        self.window / self.capacity + 1
    }
}

struct Authority<'a> {
    global: Limit,
    user: Option<Limit>,
    login: Limit,
    api_window: u64,
    rules: &'a Map<String, Value>,
}

/// 校验固定节奏的精确字段、摘要、operation 名称与有界安全整数。
pub(crate) fn validate_contract(value: &Value) -> Result<(), &'static str> {
    pacing(value).map(|_| ())
}

/// 只匹配规则自身的键，按方法和路径、路径、方法的优先级返回。
pub(crate) fn matched_rule(operation: &Value, rules: &Value) -> Option<String> {
    matching_rule(operation, rules.as_object()?)
}

/// 根据实际全局、用户、接口与登录限流校验固定节奏，不执行等待或请求。
pub(crate) fn validate_authority(
    value: &Value,
    authority: &Value,
    catalog: &BTreeMap<String, Value>,
) -> Result<(), &'static str> {
    let pacing = pacing(value)?;
    let authority_source = authority;
    let authority = limits(authority)?;
    let minimum = authority
        .global
        .spacing()
        .max(authority.user.map_or(0, Limit::spacing));
    let window = authority
        .global
        .window
        .max(authority.api_window)
        .max(authority.login.window)
        .max(authority.user.map_or(0, |user| user.window));
    if pacing.request < minimum || pacing.prepare < window + WINDOW_MARGIN_MS {
        return Err(REQUEST_SPACING);
    }
    if pacing
        .operations
        .keys()
        .any(|name| !catalog.contains_key(name))
    {
        return Err(UNKNOWN_OPERATION);
    }
    for rule in authority.rules.keys() {
        if !catalog.values().any(|operation| {
            operation_rules(operation)
                .is_some_and(|rules| rules.iter().any(|candidate| candidate == rule))
        }) {
            return Err(UNKNOWN_RULE);
        }
    }
    for (name, operation) in catalog {
        let rule = matched_rule(operation, &authority_source["api"]["rules"]);
        let mut required = 0;
        if let Some(rule) = rule.filter(|rule| !rule.is_empty()) {
            let capacity = safe_integer(&authority.rules[&rule]).ok_or(API_AUTHORITY)?;
            required = Limit {
                window: authority.api_window,
                capacity,
            }
            .spacing();
        }
        if name == "post_auth_login" {
            required = required.max(authority.login.spacing());
        }
        let interval = pacing.operations.get(name).and_then(interval).unwrap_or(0);
        if pacing.request.max(interval) < required {
            return Err(OPERATION_SPACING);
        }
    }
    Ok(())
}

fn pacing(value: &Value) -> Result<Pacing<'_>, &'static str> {
    let object = value.as_object().ok_or(CONTRACT)?;
    let fields = [
        "authority_sha256",
        "operation_intervals_ms",
        "request_interval_ms",
        "sample_prepare_wait_ms",
    ];
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(CONTRACT);
    }
    if !digest(&object["authority_sha256"]) {
        return Err(CONTRACT);
    }
    let request = interval(&object["request_interval_ms"]).ok_or(CONTRACT)?;
    let prepare = interval(&object["sample_prepare_wait_ms"]).ok_or(CONTRACT)?;
    let operations = object["operation_intervals_ms"]
        .as_object()
        .ok_or(CONTRACT)?;
    if operations
        .iter()
        .any(|(name, value)| !operation_name(name) || interval(value).is_none())
    {
        return Err(CONTRACT);
    }
    Ok(Pacing {
        request,
        prepare,
        operations,
    })
}

fn limits(value: &Value) -> Result<Authority<'_>, &'static str> {
    if value["format_version"].as_f64() != Some(1.0)
        || !value["api"]["rules"].is_object()
        || !value["user"]["enabled"].is_boolean()
    {
        return Err(AUTHORITY_SHAPE);
    }
    let global = limit(&value["global"]).ok_or(AUTHORITY_LIMIT)?;
    let user = limit(&value["user"]).ok_or(AUTHORITY_LIMIT)?;
    let login = limit(&value["login"]).ok_or(AUTHORITY_LIMIT)?;
    let api_window = interval(&value["api"]["window_ms"]).ok_or(API_AUTHORITY)?;
    let rules = value["api"]["rules"].as_object().ok_or(AUTHORITY_SHAPE)?;
    if rules.values().any(|value| safe_integer(value).is_none()) {
        return Err(API_AUTHORITY);
    }
    Ok(Authority {
        global,
        user: (value["user"]["enabled"] == true).then_some(user),
        login,
        api_window,
        rules,
    })
}

fn limit(value: &Value) -> Option<Limit> {
    Some(Limit {
        window: interval(&value["window_ms"])?,
        capacity: safe_integer(&value["capacity"])?,
    })
}

fn safe_integer(value: &Value) -> Option<u64> {
    // 先按 JS Number 取值，保留 1.0、指数写法及 JSON 数字舍入后的整数语义。
    let number = value.as_f64()?;
    (number.is_finite() && number.fract() == 0.0 && (1.0..=MAX_SAFE_INTEGER).contains(&number))
        .then_some(number as u64)
}

fn interval(value: &Value) -> Option<u64> {
    safe_integer(value).filter(|value| *value <= MAX_INTERVAL_MS)
}

fn operation_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn operation_rules(operation: &Value) -> Option<[String; 3]> {
    let method = operation["method"].as_str()?;
    let path = operation["path"].as_str()?;
    Some([
        format!("{method} {path}"),
        path.to_owned(),
        method.to_owned(),
    ])
}

fn matching_rule(operation: &Value, rules: &Map<String, Value>) -> Option<String> {
    operation_rules(operation)?
        .into_iter()
        .find(|rule| rules.contains_key(rule))
}
