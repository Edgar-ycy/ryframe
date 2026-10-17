use std::{collections::BTreeMap, fs, path::Path};

use serde::Serialize;
use toml::{Table, Value};

pub(crate) const LOGIN_RULE: &str = "POST /api/v1/auth/login";
pub(crate) const DEFAULT_LOGIN_CAPACITY: u32 = 5;
pub(crate) const DEFAULT_API_WINDOW_SECS: u64 = 60;

const DEFAULT_CAPACITY: i64 = 100;
const DEFAULT_WINDOW_SECS: i64 = 60;
const DEFAULT_USER_CAPACITY: i64 = 500;
const MAX_RULE_CAPACITY: i64 = u32::MAX as i64;
const MAX_WINDOW_SECS: i64 = 86_400;
const MAX_LOGIN_CAPACITY: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateLimitError {
    code: &'static str,
    message: &'static str,
}

impl RateLimitError {
    pub(crate) const fn code(self) -> &'static str {
        self.code
    }

    pub(crate) const fn message(self) -> &'static str {
        self.message
    }
}

const ENVIRONMENT_INVALID: RateLimitError = RateLimitError {
    code: "config_environment_invalid",
    message: "必须显式设置 APP_ENV=dev 或 APP_ENV=test",
};
const DIRECTORY_INVALID: RateLimitError = RateLimitError {
    code: "config_directory_invalid",
    message: "APP_CONFIG_DIR 不能为空",
};
const FILE_MISSING: RateLimitError = RateLimitError {
    code: "config_file_missing",
    message: "配置目录中缺少 app.toml；请检查 APP_CONFIG_DIR",
};
const FILE_UNREADABLE: RateLimitError = RateLimitError {
    code: "config_file_unreadable",
    message: "无法读取 APP 配置；请检查文件类型与读取权限",
};
const TOML_INVALID: RateLimitError = RateLimitError {
    code: "config_toml_invalid",
    message: "APP 配置不是有效 TOML；请检查 app.toml 与环境覆盖文件",
};
const TABLE_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_table_invalid",
    message: "rate_limit 与 rate_limit.api_limits 必须是配置表",
};
const BOOLEAN_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_boolean_invalid",
    message: "rate_limit 布尔字段必须使用 TOML 布尔值或 true/false 环境值",
};
const INTEGER_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_integer_invalid",
    message: "rate_limit 数值字段必须是在允许范围内的正整数",
};
const LIMIT_DISABLED: RateLimitError = RateLimitError {
    code: "rate_limit_disabled",
    message: "真实浏览器验收必须保留服务端限流（rate_limit.enabled=true）",
};
const RULE_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_rule_invalid",
    message: "rate_limit.api_limits 的规则名和正整数上限无效",
};
const LOGIN_LIMIT_INVALID: RateLimitError = RateLimitError {
    code: "login_limit_invalid",
    message: "登录接口限流容量必须是 1 到 10000 的整数",
};
const LOGIN_BUDGET_PATH_INVALID: RateLimitError = RateLimitError {
    code: "login_budget_path_invalid",
    message: "登录预算账本必须使用无换行、无 NUL 的绝对文件路径",
};
pub(crate) const RESULT_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_result_invalid",
    message: "无法把限流配置转换为本地 JSON 结果",
};

pub(crate) fn load_app_table(
    backend: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<Table, RateLimitError> {
    let app_env = environment.get("APP_ENV").ok_or(ENVIRONMENT_INVALID)?;
    if !matches!(app_env.as_str(), "dev" | "test") {
        return Err(ENVIRONMENT_INVALID);
    }
    let directory = environment
        .get("APP_CONFIG_DIR")
        .map(String::as_str)
        .unwrap_or("config");
    if directory.trim().is_empty() {
        return Err(DIRECTORY_INVALID);
    }
    let config = Path::new(directory);
    let config = if config.is_absolute() {
        config.to_path_buf()
    } else {
        backend.join(config)
    };
    let mut table = read_table(&config.join("app.toml"))?;
    let override_path = config.join(format!("app.{app_env}.toml"));
    match fs::read_to_string(&override_path) {
        Ok(source) => {
            let override_table = parse_table(&source)?;
            merge(&mut table, &override_table);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(FILE_UNREADABLE),
    }
    Ok(table)
}

pub(crate) fn settings(
    backend: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<Table, RateLimitError> {
    let table = load_app_table(backend, environment)?;
    let mut rate_limit = match table.get("rate_limit") {
        Some(value) => value.as_table().cloned().ok_or(TABLE_INVALID)?,
        None => Table::new(),
    };
    let enabled = boolean(
        &mut rate_limit,
        "enabled",
        true,
        "APP_RATE_LIMIT_ENABLED",
        environment,
    )?;
    boolean(
        &mut rate_limit,
        "enable_user_rate_limit",
        false,
        "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT",
        environment,
    )?;
    if !enabled {
        return Err(LIMIT_DISABLED);
    }
    validate_api_limits(&mut rate_limit)?;
    for (key, default, maximum, variable) in [
        (
            "capacity",
            DEFAULT_CAPACITY,
            MAX_RULE_CAPACITY,
            "APP_RATE_LIMIT_CAPACITY",
        ),
        (
            "window_secs",
            DEFAULT_WINDOW_SECS,
            MAX_WINDOW_SECS,
            "APP_RATE_LIMIT_WINDOW_SECS",
        ),
        (
            "user_capacity",
            DEFAULT_USER_CAPACITY,
            MAX_RULE_CAPACITY,
            "APP_RATE_LIMIT_USER_CAPACITY",
        ),
        (
            "user_window_secs",
            DEFAULT_WINDOW_SECS,
            MAX_WINDOW_SECS,
            "APP_RATE_LIMIT_USER_WINDOW_SECS",
        ),
        (
            "api_window_secs",
            DEFAULT_API_WINDOW_SECS as i64,
            MAX_WINDOW_SECS,
            "APP_RATE_LIMIT_API_WINDOW_SECS",
        ),
    ] {
        integer(
            &mut rate_limit,
            key,
            default,
            maximum,
            variable,
            environment,
        )?;
    }
    Ok(rate_limit)
}

pub(crate) fn login_rate_limit(
    backend: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<LoginRateLimit, RateLimitError> {
    let settings = settings(backend, environment)?;
    let capacity = api_limits(&settings)
        .get(LOGIN_RULE)
        .and_then(Value::as_integer)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(DEFAULT_LOGIN_CAPACITY);
    if !(1..=MAX_LOGIN_CAPACITY).contains(&capacity) {
        return Err(LOGIN_LIMIT_INVALID);
    }
    let window_secs = setting_integer(&settings, "api_window_secs")? as u64;
    Ok(LoginRateLimit {
        capacity,
        window_secs,
    })
}

pub(crate) fn authority(
    backend: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<Authority, RateLimitError> {
    let settings = settings(backend, environment)?;
    let rules = api_limits(&settings)
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                u32::try_from(value.as_integer().ok_or(RULE_INVALID)?).map_err(|_| RULE_INVALID)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, RateLimitError>>()?;
    Ok(Authority {
        format_version: 1,
        global: Capacity {
            capacity: setting_integer(&settings, "capacity")? as u32,
            window_ms: setting_integer(&settings, "window_secs")? as u64 * 1000,
        },
        user: UserAuthority {
            enabled: setting_boolean(&settings, "enable_user_rate_limit")?,
            capacity: setting_integer(&settings, "user_capacity")? as u32,
            window_ms: setting_integer(&settings, "user_window_secs")? as u64 * 1000,
        },
        api: ApiAuthority {
            rules: rules.clone(),
            window_ms: setting_integer(&settings, "api_window_secs")? as u64 * 1000,
        },
        login: Capacity {
            capacity: rules
                .get(LOGIN_RULE)
                .copied()
                .unwrap_or(DEFAULT_LOGIN_CAPACITY),
            window_ms: setting_integer(&settings, "api_window_secs")? as u64 * 1000,
        },
    })
}

pub(crate) fn login_budget_environment(
    backend: &Path,
    environment: &BTreeMap<String, String>,
    state_path: &str,
) -> Result<BTreeMap<String, String>, RateLimitError> {
    let path = Path::new(state_path);
    if !path.is_absolute()
        || state_path
            .chars()
            .any(|character| matches!(character, '\r' | '\n' | '\0'))
    {
        return Err(LOGIN_BUDGET_PATH_INVALID);
    }
    let limits = login_rate_limit(backend, environment)?;
    Ok(BTreeMap::from([
        (
            "RYFRAME_E2E_LOGIN_BUDGET_STATE".to_owned(),
            state_path.to_owned(),
        ),
        (
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY".to_owned(),
            limits.capacity.to_string(),
        ),
        (
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS".to_owned(),
            limits.window_secs.to_string(),
        ),
    ]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoginRateLimit {
    pub(crate) capacity: u32,
    pub(crate) window_secs: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Authority {
    format_version: u8,
    global: Capacity,
    user: UserAuthority,
    api: ApiAuthority,
    login: Capacity,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Capacity {
    capacity: u32,
    window_ms: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct UserAuthority {
    enabled: bool,
    capacity: u32,
    window_ms: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ApiAuthority {
    rules: BTreeMap<String, u32>,
    window_ms: u64,
}

fn read_table(path: &Path) -> Result<Table, RateLimitError> {
    let source = fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FILE_MISSING
        } else {
            FILE_UNREADABLE
        }
    })?;
    parse_table(&source)
}

fn parse_table(source: &str) -> Result<Table, RateLimitError> {
    toml::from_str(source).map_err(|_| TOML_INVALID)
}

fn merge(base: &mut Table, override_table: &Table) {
    for (key, value) in override_table {
        if let (Some(Value::Table(base)), Value::Table(override_table)) = (base.get_mut(key), value)
        {
            merge(base, override_table);
            continue;
        }
        base.insert(key.clone(), value.clone());
    }
}

fn boolean(
    table: &mut Table,
    key: &str,
    default: bool,
    variable: &str,
    environment: &BTreeMap<String, String>,
) -> Result<bool, RateLimitError> {
    let value = match environment.get(variable) {
        Some(raw) => match raw.as_str() {
            "true" => true,
            "false" => false,
            _ => return Err(BOOLEAN_INVALID),
        },
        None => table
            .get(key)
            .cloned()
            .unwrap_or(Value::Boolean(default))
            .as_bool()
            .ok_or(BOOLEAN_INVALID)?,
    };
    table.insert(key.to_owned(), Value::Boolean(value));
    Ok(value)
}

fn integer(
    table: &mut Table,
    key: &str,
    default: i64,
    maximum: i64,
    variable: &str,
    environment: &BTreeMap<String, String>,
) -> Result<i64, RateLimitError> {
    let value = match environment.get(variable) {
        Some(raw) => parse_environment_integer(raw, maximum)?,
        None => validate_integer(
            table.get(key).cloned().unwrap_or(Value::Integer(default)),
            maximum,
        )?,
    };
    table.insert(key.to_owned(), Value::Integer(value));
    Ok(value)
}

fn validate_api_limits(rate_limit: &mut Table) -> Result<(), RateLimitError> {
    let mut limits = match rate_limit.get("api_limits") {
        Some(value) => value.as_table().cloned().ok_or(TABLE_INVALID)?,
        None => Table::new(),
    };
    for (rule, value) in &limits {
        if rule.trim().is_empty() || validate_integer(value.clone(), MAX_RULE_CAPACITY).is_err() {
            return Err(RULE_INVALID);
        }
    }
    rate_limit.insert(
        "api_limits".to_owned(),
        Value::Table(std::mem::take(&mut limits)),
    );
    Ok(())
}

fn api_limits(settings: &Table) -> &Table {
    settings
        .get("api_limits")
        .and_then(Value::as_table)
        .expect("settings 必须先验证 api_limits")
}

fn setting_boolean(table: &Table, key: &str) -> Result<bool, RateLimitError> {
    table
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(BOOLEAN_INVALID)
}

fn setting_integer(table: &Table, key: &str) -> Result<i64, RateLimitError> {
    table
        .get(key)
        .and_then(Value::as_integer)
        .ok_or(RESULT_INVALID)
}

fn validate_integer(value: Value, maximum: i64) -> Result<i64, RateLimitError> {
    match value {
        Value::Integer(value) if (1..=maximum).contains(&value) => Ok(value),
        _ => Err(INTEGER_INVALID),
    }
}

fn parse_environment_integer(raw: &str, maximum: i64) -> Result<i64, RateLimitError> {
    let digits = raw.strip_prefix('+').unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(INTEGER_INVALID);
    }
    let value = digits.parse::<i64>().map_err(|_| INTEGER_INVALID)?;
    if !(1..=maximum).contains(&value) {
        return Err(INTEGER_INVALID);
    }
    Ok(value)
}
