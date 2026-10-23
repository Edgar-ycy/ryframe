use crate::{Result, recovery::clone::native::json as native_json};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, io::Read};

const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u8,
    operation: String,
    arguments: Value,
    write: bool,
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = env::args_os().skip(1);
    if args.next()?.as_os_str() != "__fixture-environment" {
        return None;
    }
    let mut bytes = Vec::new();
    let result = std::io::stdin()
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes);
    let (code, response) = if result.is_err() || args.next().is_some() {
        failure("夹具环境协议读取失败；请核对正式工具绑定")
    } else {
        execute(&bytes)
    };
    println!("{response}");
    Some(code)
}

pub(crate) fn execute(bytes: &[u8]) -> (i32, Value) {
    match dispatch(bytes) {
        Ok(value) if value.to_string().len() <= LIMIT => (0, json!({"ok":true,"result":value})),
        Ok(_) => failure("夹具环境响应过长；保留现场，禁止自动重放"),
        Err(error) => {
            let message = if let Some(error) = error.downcast_ref::<super::output::OutputError>() {
                error.to_string()
            } else {
                error.downcast_ref::<GuardError>().map_or(
                    "夹具环境请求未完成；核对字段、路径、来源摘要和正式工具绑定；保留现场，禁止自动重放",
                    |error| error.0,
                ).to_owned()
            };
            failure(&message)
        }
    }
}

#[derive(Debug)]
pub(super) struct GuardError(pub(super) &'static str);
impl std::fmt::Display for GuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for GuardError {}
pub(super) fn check(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Box::new(GuardError(message)))
    }
}
fn failure(message: &str) -> (i32, Value) {
    (
        2,
        json!({"ok":false,"error":{"code":"invalid_fixture_environment","message":message,
        "automatic_retry_permitted":false,"remote_write_may_have_occurred":false}}),
    )
}
fn dispatch(bytes: &[u8]) -> Result<Value> {
    check(
        !bytes.is_empty() && bytes.len() <= LIMIT,
        "夹具环境协议为空或超过 4 MiB",
    )?;
    let raw = std::str::from_utf8(bytes)?;
    native_json::parse_fragment(raw, "夹具环境请求")?;
    let request: Request = serde_json::from_str(raw)?;
    check(
        request.format_version == 1 && !request.write,
        "夹具环境守卫协议只允许版本 1 的只读请求",
    )?;
    let args = &request.arguments;
    match request.operation.as_str() {
        "fixture" => {
            super::guards::fixture(args)?;
            Ok(json!({}))
        }
        "maintenance" => {
            super::guards::maintenance(args)?;
            Ok(json!({}))
        }
        "reset-password" => {
            super::guards::reset_password(args)?;
            Ok(json!({}))
        }
        "preflight-binding" => super::guards::preflight_binding(args),
        "preflight-successor" => {
            native_json::exact(args, &["previous", "review"], "夹具继承请求")?;
            super::guards::successor(&args["previous"], &args["review"])?;
            Ok(json!({}))
        }
        "configuration-settings" => super::configuration::settings(args),
        "configuration-text" => super::configuration::text(args),
        "administrator-credentials" => super::configuration::administrator(args),
        "plan" => super::plan::create(args),
        "planned-inputs" => super::plan::inputs(args),
        "prepare-output" => super::output::request(args),
        _ => Err(Box::new(GuardError("未知夹具环境操作"))),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/environment/tests.rs"]
mod tests;
