use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::config::{self, RateLimitError};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TableRequest {
    backend: PathBuf,
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBudgetRequest {
    backend: PathBuf,
    environment: BTreeMap<String, String>,
    state_path: String,
}

pub(super) fn execute(arguments: &[OsString], input: &[u8]) -> (i32, String) {
    let result = execute_inner(arguments, input);
    match result {
        Ok(result) => match serde_json::to_string(&Success { ok: true, result }) {
            Ok(output) => (0, output),
            Err(_) => (1, failure_text(config::RESULT_INVALID)),
        },
        Err(failure) => failure.response(),
    }
}

pub(super) fn invalid_request() -> (i32, String) {
    Failure::Parameter.response()
}

fn execute_inner(arguments: &[OsString], input: &[u8]) -> Result<Value, Failure> {
    let operation = match arguments {
        [operation] => operation.to_str().ok_or(Failure::Parameter)?,
        _ => return Err(Failure::Parameter),
    };
    match operation {
        "authority" => {
            let request = decode::<TableRequest>(input)?;
            config::authority(&request.backend, &request.environment)
                .and_then(to_value)
                .map_err(Failure::Config)
        }
        "load-table" => {
            let request = decode::<TableRequest>(input)?;
            config::load_app_table(&request.backend, &request.environment)
                .and_then(to_value)
                .map_err(Failure::Config)
        }
        "settings" => {
            let request = decode::<TableRequest>(input)?;
            config::settings(&request.backend, &request.environment)
                .and_then(to_value)
                .map_err(Failure::Config)
        }
        "login-budget" => {
            let request = decode::<LoginBudgetRequest>(input)?;
            config::login_budget_environment(
                &request.backend,
                &request.environment,
                &request.state_path,
            )
            .and_then(to_value)
            .map_err(Failure::Config)
        }
        _ => Err(Failure::Parameter),
    }
}

fn decode<T: DeserializeOwned>(input: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(input).map_err(|_| Failure::Parameter)
}

fn to_value<T: Serialize>(value: T) -> Result<Value, RateLimitError> {
    serde_json::to_value(value).map_err(|_| config::RESULT_INVALID)
}

#[derive(Debug)]
enum Failure {
    Parameter,
    Config(RateLimitError),
}

impl Failure {
    fn response(self) -> (i32, String) {
        match self {
            Self::Parameter => (
                2,
                failure_text_with(
                    "invalid_request",
                    "请求必须仅包含 backend、environment 及操作所需字段",
                ),
            ),
            Self::Config(error) => (1, failure_text(error)),
        }
    }
}

#[derive(Serialize)]
struct Success {
    ok: bool,
    result: Value,
}

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

fn failure_text(error: RateLimitError) -> String {
    failure_text_with(error.code(), error.message())
}

fn failure_text_with(code: &'static str, message: &'static str) -> String {
    serde_json::to_string(&ErrorResponse {
        ok: false,
        error: ErrorBody { code, message },
    })
    .unwrap_or_else(|_| {
        r#"{"ok":false,"error":{"code":"rate_limit_result_invalid","message":"无法把限流配置转换为本地 JSON 结果"}}"#
            .to_owned()
    })
}
