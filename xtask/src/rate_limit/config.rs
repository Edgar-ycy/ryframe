use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ryframe_config::{AppConfig, Environment};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RateLimitError {
    code: &'static str,
    message: Cow<'static, str>,
}

impl RateLimitError {
    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        self.message.as_ref()
    }

    fn at_field(mut self, field: &str) -> Self {
        self.message = Cow::Owned(format!("{field}：{}；请修正该 TOML 字段", self.message));
        self
    }

    fn at_source(mut self, directory: &Path, app_env: &str) -> Self {
        self.message = Cow::Owned(format!(
            "配置文件 {} 与 {}：{}",
            directory.join("app.toml").display(),
            directory.join(format!("app.{app_env}.toml")).display(),
            self.message
        ));
        self
    }
}

const ENVIRONMENT_INVALID: RateLimitError = RateLimitError {
    code: "config_environment_invalid",
    message: Cow::Borrowed("app_env 必须显式设置为 dev 或 test；请修正请求字段"),
};
const DIRECTORY_INVALID: RateLimitError = RateLimitError {
    code: "config_directory_invalid",
    message: Cow::Borrowed("backend/config_dir 必须非空且不含控制字符；请修正请求路径"),
};
const DOCUMENT_INVALID: RateLimitError = RateLimitError {
    code: "config_document_invalid",
    message: Cow::Borrowed("应用配置文档读取或预检查失败；请修正所选配置文件"),
};
const TABLE_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_table_invalid",
    message: Cow::Borrowed("rate_limit 与 rate_limit.api_limits 必须是配置表"),
};
const BOOLEAN_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_boolean_invalid",
    message: Cow::Borrowed("rate_limit 布尔字段必须使用 TOML 布尔值"),
};
const INTEGER_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_integer_invalid",
    message: Cow::Borrowed("rate_limit 数值字段必须是在允许范围内的正整数"),
};
const LIMIT_DISABLED: RateLimitError = RateLimitError {
    code: "rate_limit_disabled",
    message: Cow::Borrowed("真实浏览器验收必须保留服务端限流（rate_limit.enabled=true）"),
};
const RULE_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_rule_invalid",
    message: Cow::Borrowed("rate_limit.api_limits 的规则名和正整数上限无效"),
};
pub(crate) const RESULT_INVALID: RateLimitError = RateLimitError {
    code: "rate_limit_result_invalid",
    message: Cow::Borrowed("无法把限流配置转换为本地 JSON 结果"),
};

pub(crate) fn load_app_table(
    backend: &Path,
    config_dir: Option<&Path>,
    app_env: &str,
) -> Result<Table, RateLimitError> {
    let environment = match app_env {
        "dev" => Environment::Dev,
        "test" => Environment::Test,
        _ => return Err(ENVIRONMENT_INVALID),
    };
    let directory = configuration_directory(backend, config_dir)?;
    AppConfig::read_document(&directory, environment).map_err(|error| RateLimitError {
        message: Cow::Owned(error.to_string()),
        ..DOCUMENT_INVALID
    })
}

fn configuration_directory(
    backend: &Path,
    config_dir: Option<&Path>,
) -> Result<PathBuf, RateLimitError> {
    let directory = config_dir.unwrap_or_else(|| Path::new("config"));
    for path in [backend, directory] {
        let value = path.to_str().ok_or(DIRECTORY_INVALID)?;
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(DIRECTORY_INVALID);
        }
    }
    Ok(if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        backend.join(directory)
    })
}

pub(crate) fn settings(
    backend: &Path,
    config_dir: Option<&Path>,
    app_env: &str,
) -> Result<Table, RateLimitError> {
    let table = load_app_table(backend, config_dir, app_env)?;
    let directory = configuration_directory(backend, config_dir)?;
    table_settings(&table).map_err(|error| error.at_source(&directory, app_env))
}

fn table_settings(table: &Table) -> Result<Table, RateLimitError> {
    let mut rate_limit = match table.get("rate_limit") {
        Some(value) => value
            .as_table()
            .cloned()
            .ok_or_else(|| TABLE_INVALID.at_field("rate_limit"))?,
        None => Table::new(),
    };
    let enabled = boolean(&mut rate_limit, "enabled", true)?;
    boolean(&mut rate_limit, "enable_user_rate_limit", false)?;
    if !enabled {
        return Err(LIMIT_DISABLED.at_field("rate_limit.enabled"));
    }
    validate_api_limits(&mut rate_limit)?;
    for (key, default, maximum) in [
        ("capacity", DEFAULT_CAPACITY, MAX_RULE_CAPACITY),
        ("window_secs", DEFAULT_WINDOW_SECS, MAX_WINDOW_SECS),
        ("user_capacity", DEFAULT_USER_CAPACITY, MAX_RULE_CAPACITY),
        ("user_window_secs", DEFAULT_WINDOW_SECS, MAX_WINDOW_SECS),
        (
            "api_window_secs",
            DEFAULT_API_WINDOW_SECS as i64,
            MAX_WINDOW_SECS,
        ),
    ] {
        integer(&mut rate_limit, key, default, maximum)?;
    }
    Ok(rate_limit)
}
pub(crate) fn authority(
    backend: &Path,
    config_dir: Option<&Path>,
    app_env: &str,
) -> Result<Authority, RateLimitError> {
    let settings = settings(backend, config_dir, app_env)?;
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

fn boolean(table: &mut Table, key: &str, default: bool) -> Result<bool, RateLimitError> {
    let value = table
        .get(key)
        .cloned()
        .unwrap_or(Value::Boolean(default))
        .as_bool()
        .ok_or_else(|| BOOLEAN_INVALID.at_field(&format!("rate_limit.{key}")))?;
    table.insert(key.to_owned(), Value::Boolean(value));
    Ok(value)
}

fn integer(
    table: &mut Table,
    key: &str,
    default: i64,
    maximum: i64,
) -> Result<i64, RateLimitError> {
    let value = validate_integer(
        table.get(key).cloned().unwrap_or(Value::Integer(default)),
        maximum,
    )
    .map_err(|error| error.at_field(&format!("rate_limit.{key}")))?;
    table.insert(key.to_owned(), Value::Integer(value));
    Ok(value)
}
fn validate_api_limits(rate_limit: &mut Table) -> Result<(), RateLimitError> {
    let mut limits = match rate_limit.get("api_limits") {
        Some(value) => value
            .as_table()
            .cloned()
            .ok_or_else(|| TABLE_INVALID.at_field("rate_limit.api_limits"))?,
        None => Table::new(),
    };
    for (rule, value) in &limits {
        if rule.trim().is_empty() || validate_integer(value.clone(), MAX_RULE_CAPACITY).is_err() {
            return Err(RULE_INVALID.at_field("rate_limit.api_limits"));
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
